//! Editing commands. Each one is undoable ([`crate::history`]); errors
//! explain why an edit isn't possible (locked slide, field text...).

use std::sync::Arc;

use anyhow::{Context as _, bail};
use docs_engine::{Rgb, Selection, Toggle};

use crate::history::{Command, Focus, Kind};
use crate::model::*;
use crate::presentation::{Presentation, Slide, SlideSrc};
use crate::template;
use crate::text_ops;
use crate::xml::*;

/// A shape on a slide: `(slide index, shape index)`.
pub type ShapeRef = (usize, usize);

/// Relationship kinds a duplicated slide may share with its original.
const SHAREABLE_RELS: [&str; 7] = [
    "slideLayout",
    "image",
    "hyperlink",
    "slide",
    "media",
    "audio",
    "video",
];

/// Width and height in pixels of a PNG or JPEG.
pub fn image_size(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") && bytes.len() >= 24 {
        let w = u32::from_be_bytes(bytes[16..20].try_into().ok()?);
        let h = u32::from_be_bytes(bytes[20..24].try_into().ok()?);
        return Some((w, h));
    }
    if bytes.starts_with(&[0xFF, 0xD8]) {
        let mut i = 2;
        while i + 9 < bytes.len() {
            if bytes[i] != 0xFF {
                i += 1;
                continue;
            }
            let marker = bytes[i + 1];
            let len = u16::from_be_bytes([bytes[i + 2], bytes[i + 3]]) as usize;
            if matches!(marker, 0xC0..=0xCF) && !matches!(marker, 0xC4 | 0xC8 | 0xCC) {
                let h = u16::from_be_bytes([bytes[i + 5], bytes[i + 6]]) as u32;
                let w = u16::from_be_bytes([bytes[i + 7], bytes[i + 8]]) as u32;
                return Some((w, h));
            }
            i += 2 + len;
        }
    }
    None
}

/// `png` / `jpeg` from the file's signature.
pub fn image_ext(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG") {
        Some("png")
    } else if bytes.starts_with(&[0xFF, 0xD8]) {
        Some("jpeg")
    } else {
        None
    }
}

impl Presentation {
    // ── command plumbing ───────────────────────────────────────────────────

    pub(crate) fn slide_mut(&mut self, i: usize) -> anyhow::Result<&mut Slide> {
        let s = self.slides.get_mut(i).context("no such slide")?;
        if let Some(why) = &s.locked {
            bail!("This slide can't be edited: {why}");
        }
        Ok(Arc::make_mut(s))
    }

    /// Run an edit and record it if it changed anything.
    pub(crate) fn record<R>(
        &mut self,
        kind: Kind,
        target: Option<ShapeRef>,
        before: Focus,
        boundary: bool,
        f: impl FnOnce(&mut Self) -> anyhow::Result<(R, Focus)>,
    ) -> anyhow::Result<R> {
        let snapshot = self.slides.clone();
        let (r, after) = match f(self) {
            Ok(v) => v,
            Err(e) => {
                self.slides = snapshot;
                return Err(e);
            }
        };
        let changed = snapshot.len() != self.slides.len()
            || snapshot
                .iter()
                .zip(&self.slides)
                .any(|(a, b)| !Arc::ptr_eq(a, b) && a != b);
        if changed {
            let cmd = Command::new(snapshot, self.slides.clone(), before, after, kind, target);
            self.history.push(cmd, boundary);
        }
        Ok(r)
    }

    pub fn can_undo(&self) -> bool {
        self.history.can_undo()
    }

    pub fn can_redo(&self) -> bool {
        self.history.can_redo()
    }

    /// Undo the last edit; returns where it happened.
    pub fn undo(&mut self) -> Option<Focus> {
        let cmd = self.history.undo()?;
        let (slides, focus) = (cmd.before.clone(), cmd.focus_before.clone());
        self.slides = slides;
        Some(focus)
    }

    pub fn redo(&mut self) -> Option<Focus> {
        let cmd = self.history.redo()?;
        let (slides, focus) = (cmd.after.clone(), cmd.focus_after.clone());
        self.slides = slides;
        Some(focus)
    }

    /// End typing/IME coalescing (e.g. when the caret moves by mouse).
    pub fn seal_history(&mut self) {
        self.history.seal();
    }

    // ── shapes ─────────────────────────────────────────────────────────────

    /// Move / resize a shape. `nudge` merges repeated arrow-key moves.
    pub fn set_xfrm(&mut self, r: ShapeRef, xfrm: Xfrm, nudge: bool) -> anyhow::Result<()> {
        let kind = if nudge { Kind::Nudge } else { Kind::Other };
        let focus = Focus::shape(r.0, r.1);
        self.record(kind, Some(r), focus.clone(), false, |p| {
            let s = p.slide_mut(r.0)?;
            let shape = s.shapes.get_mut(r.1).context("no such shape")?;
            if !shape.movable() {
                bail!("This object can't be moved yet");
            }
            let xfrm = Xfrm {
                cx: xfrm.cx.max(1),
                cy: xfrm.cy.max(1),
                ..xfrm
            };
            shape.xfrm = Some(xfrm);
            Ok(((), focus))
        })
    }

    fn add_shape(&mut self, slide: usize, mut shape: Shape) -> anyhow::Result<usize> {
        self.record(Kind::Other, None, Focus::slide(slide), false, |p| {
            let s = p.slide_mut(slide)?;
            shape.id = s.next_shape_id();
            shape.name = format!("{} {}", shape.name, shape.id - 1);
            s.shapes.push(shape);
            let i = s.shapes.len() - 1;
            Ok((i, Focus::shape(slide, i)))
        })
    }

    fn centered(&self, cx: Emu, cy: Emu) -> Xfrm {
        Xfrm::new((self.size.cx - cx) / 2, (self.size.cy - cy) / 2, cx, cy)
    }

    /// Insert an empty text box in the middle of the slide.
    pub fn add_text_box(&mut self, slide: usize) -> anyhow::Result<usize> {
        let xfrm = self.centered(self.size.cx / 2, 369_332);
        self.add_shape(
            slide,
            Shape {
                id: 0,
                name: "TextBox".into(),
                kind: ShapeKind::Shape {
                    geom: Geom::Rect,
                    text_box: true,
                },
                xfrm: Some(xfrm),
                ph: None,
                look: Look::default(),
                text: Some(TextBody::new(vec![TextPara::default()])),
                src: None,
            },
        )
    }

    /// Insert a rectangle or ellipse (Office blue, white text).
    pub fn add_geometry(&mut self, slide: usize, geom: Geom) -> anyhow::Result<usize> {
        let xfrm = self.centered(self.size.cx / 4, self.size.cy / 4);
        let name = match geom {
            Geom::Ellipse => "Oval",
            _ => "Rectangle",
        };
        let mut body = TextBody::new(vec![TextPara {
            props: ParaProps {
                align: Some(Align::Center),
                ..ParaProps::default()
            },
            ..TextPara::default()
        }]);
        body.body.anchor = Some(Anchor::Middle);
        self.add_shape(
            slide,
            Shape {
                id: 0,
                name: name.into(),
                kind: ShapeKind::Shape {
                    geom,
                    text_box: false,
                },
                xfrm: Some(xfrm),
                ph: None,
                look: Look {
                    fill: Some(Rgb([0x44, 0x72, 0xC4])),
                    line: Some((Rgb([0x2F, 0x52, 0x8F]), 12_700)),
                    text_color: Some(Rgb([0xFF, 0xFF, 0xFF])),
                },
                text: Some(body),
                src: None,
            },
        )
    }

    /// Insert a PNG or JPEG picture, fitted into the middle of the slide.
    pub fn add_picture(&mut self, slide: usize, bytes: Vec<u8>) -> anyhow::Result<usize> {
        let ext = image_ext(&bytes).context("Only PNG and JPEG pictures can be inserted")?;
        let (w, h) = image_size(&bytes).unwrap_or((400, 300));
        let (w, h) = (w.max(1) as f64, h.max(1) as f64);
        let max_w = self.size.cx as f64 * 0.6;
        let max_h = self.size.cy as f64 * 0.6;
        let natural = (w * EMU_PER_PX as f64, h * EMU_PER_PX as f64);
        let k = (max_w / natural.0).min(max_h / natural.1).min(1.0);
        let xfrm = self.centered((natural.0 * k) as Emu, (natural.1 * k) as Emu);
        let mut n = 1;
        let part = loop {
            let p = format!("ppt/media/image{n}.{ext}");
            if !self.pkg.names.contains(&p) && !self.media.contains_key(&p) {
                break p;
            }
            n += 1;
        };
        self.media.insert(part.clone(), Arc::new(bytes));
        self.record(Kind::Other, None, Focus::slide(slide), false, |p| {
            let s = p.slide_mut(slide)?;
            let rel = next_rel_id(&s.rels);
            Arc::make_mut(&mut s.rels).push(Rel {
                id: rel.clone(),
                ty: rel_type("image"),
                target: relative(&s.part, &part),
                external: false,
            });
            let id = s.next_shape_id();
            s.shapes.push(Shape {
                id,
                name: format!("Picture {}", id - 1),
                kind: ShapeKind::Picture {
                    rel,
                    part: Some(part.clone()),
                },
                xfrm: Some(xfrm),
                ph: None,
                look: Look::default(),
                text: None,
                src: None,
            });
            let i = s.shapes.len() - 1;
            Ok((i, Focus::shape(slide, i)))
        })
    }

    pub fn delete_shape(&mut self, r: ShapeRef) -> anyhow::Result<()> {
        self.record(Kind::Other, None, Focus::shape(r.0, r.1), false, |p| {
            let s = p.slide_mut(r.0)?;
            if r.1 >= s.shapes.len() {
                bail!("no such shape");
            }
            s.shapes.remove(r.1);
            Ok(((), Focus::slide(r.0)))
        })
    }

    // ── text ───────────────────────────────────────────────────────────────

    fn edit_text(
        &mut self,
        r: ShapeRef,
        kind: Kind,
        sel: Selection,
        boundary: bool,
        f: impl FnOnce(&mut Vec<TextPara>) -> Selection,
    ) -> anyhow::Result<Selection> {
        let before = Focus::text(r.0, r.1, sel);
        self.record(kind, Some(r), before, boundary, |p| {
            let s = p.slide_mut(r.0)?;
            let shape = s.shapes.get_mut(r.1).context("no such shape")?;
            let body = shape.text.as_mut().context("This shape has no text")?;
            if let Some(why) = &body.locked {
                bail!("This text can't be edited yet: it has {why}");
            }
            let sel = f(&mut body.paras);
            Ok((sel, Focus::text(r.0, r.1, sel)))
        })
    }

    pub fn text_paras(&self, r: ShapeRef) -> Option<&[TextPara]> {
        Some(&self.slides.get(r.0)?.shapes.get(r.1)?.text.as_ref()?.paras)
    }

    /// Replace the selection with `text`; `props` overrides the formatting.
    pub fn insert_text(
        &mut self,
        r: ShapeRef,
        sel: Selection,
        text: &str,
        props: Option<RunProps>,
    ) -> anyhow::Result<Selection> {
        let boundary = text.chars().any(|c| c.is_whitespace()) || !sel.is_empty();
        let kind = if text.contains('\n') {
            Kind::Other
        } else {
            Kind::Typing
        };
        self.edit_text(r, kind, sel, boundary, |paras| {
            text_ops::insert_text(paras, sel, text, props)
        })
    }

    /// IME composition: every update and the commit are one undo step.
    pub fn compose(
        &mut self,
        r: ShapeRef,
        sel: Selection,
        text: &str,
        props: Option<RunProps>,
    ) -> anyhow::Result<Selection> {
        self.edit_text(r, Kind::Composing, sel, false, |paras| {
            text_ops::insert_text(paras, sel, text, props)
        })
    }

    pub fn backspace(&mut self, r: ShapeRef, sel: Selection) -> anyhow::Result<Selection> {
        self.edit_text(r, Kind::Deleting, sel, !sel.is_empty(), |paras| {
            text_ops::backspace(paras, sel)
        })
    }

    pub fn delete_forward(&mut self, r: ShapeRef, sel: Selection) -> anyhow::Result<Selection> {
        self.edit_text(r, Kind::Deleting, sel, !sel.is_empty(), |paras| {
            text_ops::delete_forward(paras, sel)
        })
    }

    /// Whether `toggle` is on for the whole selection (resolved through
    /// the layout and master).
    pub fn is_on(&self, r: ShapeRef, sel: Selection, toggle: Toggle) -> bool {
        let Some(shape) = self.slides.get(r.0).and_then(|s| s.shapes.get(r.1)) else {
            return false;
        };
        let Some(body) = &shape.text else {
            return false;
        };
        let (a, b) = sel.ordered();
        let flag = |props: &RunProps, para: usize| {
            let run = self.resolve_run(r.0, shape, para, props);
            match toggle {
                Toggle::Bold => run.bold,
                Toggle::Italic => run.italic,
                Toggle::Underline => run.underline,
            }
        };
        if a == b {
            let p = text_ops::clamp(&body.paras, a);
            return flag(&body.paras[p.para].props_at(p.offset), p.para);
        }
        let mut any = false;
        for i in a.para..=b.para.min(body.paras.len() - 1) {
            let p = &body.paras[i];
            let from = if i == a.para { a.offset } else { 0 };
            let to = if i == b.para { b.offset } else { p.text.len() };
            for (range, run) in p.run_ranges() {
                if range.end > from && range.start < to {
                    any = true;
                    if !flag(&run.props, i) {
                        return false;
                    }
                }
            }
        }
        any
    }

    /// Ctrl+B/I/U on a selection.
    pub fn toggle(&mut self, r: ShapeRef, sel: Selection, toggle: Toggle) -> anyhow::Result<()> {
        let on = !self.is_on(r, sel, toggle);
        self.edit_text(r, Kind::Other, sel, true, |paras| {
            text_ops::format(paras, sel, &|p| set_toggle(p, toggle, on));
            sel
        })?;
        Ok(())
    }

    /// Formatting typed text takes at `pos`.
    pub fn run_props_at(&self, r: ShapeRef, pos: docs_engine::Position) -> RunProps {
        self.text_paras(r)
            .map(|paras| {
                let p = text_ops::clamp(paras, pos);
                paras[p.para].props_at(p.offset)
            })
            .unwrap_or_default()
    }

    // ── slides ─────────────────────────────────────────────────────────────

    fn new_part(&mut self) -> String {
        let part = format!("ppt/slides/slide{}.xml", self.next_slide_num);
        self.next_slide_num += 1;
        part
    }

    fn new_sld_id(&mut self) -> u32 {
        let id = self.next_sld_id;
        self.next_sld_id += 1;
        id
    }

    /// The layout a new slide after `after` gets: the same as that slide's,
    /// except after a title slide ("Title and Content" then).
    fn layout_for_new(&self, after: Option<usize>) -> usize {
        let cur = after
            .filter(|&i| i < self.slides.len())
            .map_or(0, |i| self.slides[i].layout);
        let is_title = self.layouts.get(cur).and_then(|l| l.kind.as_deref()) == Some("title");
        if (is_title || after.is_none())
            && let Some(i) = self
                .layouts
                .iter()
                .position(|l| l.kind.as_deref() == Some("obj"))
        {
            return i;
        }
        cur.min(self.layouts.len() - 1)
    }

    /// Insert a new slide after `after` (`None` = at the start); returns its index.
    pub fn new_slide(&mut self, after: Option<usize>) -> anyhow::Result<usize> {
        let layout = self.layout_for_new(after);
        let part = self.new_part();
        let sld_id = self.new_sld_id();
        let l = &self.layouts[layout];
        let mut shapes = Vec::new();
        for ph in l.shapes.iter().filter_map(|s| s.ph.as_ref()) {
            if ph.is_footer() || ph.ty() == "pic" {
                continue;
            }
            let id = shapes.len() as u32 + 2;
            let base = match ph.ty() {
                "title" | "ctrTitle" => "Title",
                "subTitle" => "Subtitle",
                "body" => "Text Placeholder",
                _ => "Content Placeholder",
            };
            shapes.push(Shape {
                id,
                name: format!("{base} {}", id - 1),
                kind: ShapeKind::Shape {
                    geom: Geom::Rect,
                    text_box: false,
                },
                xfrm: None,
                ph: Some(ph.clone()),
                look: Look::default(),
                text: Some(TextBody::new(vec![TextPara::default()])),
                src: None,
            });
        }
        let rels = vec![Rel {
            id: "rId1".into(),
            ty: rel_type("slideLayout"),
            target: relative(&part, &l.part),
            external: false,
        }];
        let slide = Slide {
            sld_id,
            part,
            layout,
            shapes,
            bg: None,
            show_master_sp: true,
            locked: None,
            rels: Arc::new(rels),
            src: Arc::new(SlideSrc {
                prefix: template::slide_prefix(),
                suffix: template::SLIDE_SUFFIX.to_owned(),
                shapes: Vec::new(),
                rels: Vec::new(),
            }),
            origin: None,
        };
        let at = after.map_or(0, |i| (i + 1).min(self.slides.len()));
        let before = Focus::slide(after.unwrap_or(0));
        self.record(Kind::Other, None, before, false, |p| {
            p.slides.insert(at, Arc::new(slide));
            Ok((at, Focus::slide(at)))
        })
    }

    /// Copy slide `i` right after it.
    pub fn duplicate_slide(&mut self, i: usize) -> anyhow::Result<usize> {
        let src = self.slides.get(i).context("no such slide")?.clone();
        if let Some(r) = src.rels.iter().find(|r| {
            !SHAREABLE_RELS.contains(&r.kind()) && !matches!(r.kind(), "notesSlide" | "comments")
        }) {
            bail!(
                "This slide has content Kantor can't copy yet ({}), so it can't be duplicated",
                r.kind()
            );
        }
        let part = self.new_part();
        let sld_id = self.new_sld_id();
        let rels: Vec<Rel> = src
            .rels
            .iter()
            .filter(|r| SHAREABLE_RELS.contains(&r.kind()))
            .map(|r| match r.part(&src.part) {
                Some(t) => Rel {
                    target: relative(&part, &t),
                    ..r.clone()
                },
                None => r.clone(),
            })
            .collect();
        let copy = Slide {
            sld_id,
            part,
            rels: Arc::new(rels),
            origin: None,
            ..(*src).clone()
        };
        self.record(Kind::Other, None, Focus::slide(i), false, |p| {
            p.slides.insert(i + 1, Arc::new(copy));
            Ok((i + 1, Focus::slide(i + 1)))
        })
    }

    pub fn delete_slide(&mut self, i: usize) -> anyhow::Result<()> {
        let s = self.slides.get(i).context("no such slide")?.clone();
        let linked = self.slides.iter().enumerate().any(|(j, o)| {
            j != i
                && o.rels
                    .iter()
                    .any(|r| r.kind() == "slide" && r.part(&o.part).as_deref() == Some(&s.part))
        });
        if linked {
            bail!("Other slides link to this slide, so it can't be deleted yet");
        }
        self.record(Kind::Other, None, Focus::slide(i), false, |p| {
            p.slides.remove(i);
            let next = i.min(p.slides.len().saturating_sub(1));
            Ok(((), Focus::slide(next)))
        })
    }

    /// Move slide `from` to position `to`.
    pub fn move_slide(&mut self, from: usize, to: usize) -> anyhow::Result<()> {
        if from >= self.slides.len() || to >= self.slides.len() || from == to {
            return Ok(());
        }
        self.record(Kind::Other, None, Focus::slide(from), false, |p| {
            let s = p.slides.remove(from);
            p.slides.insert(to, s);
            Ok(((), Focus::slide(to)))
        })
    }
}

/// Turn a toggle on or off in `p` (an exotic underline style is replaced).
pub fn set_toggle(p: &mut RunProps, toggle: Toggle, on: bool) {
    match toggle {
        Toggle::Bold => p.bold = Some(on),
        Toggle::Italic => p.italic = Some(on),
        Toggle::Underline => {
            p.underline = Some(on);
            p.shown_underline = false;
            p.extra.attrs.retain(|(k, _)| k != "u");
        }
    }
}
