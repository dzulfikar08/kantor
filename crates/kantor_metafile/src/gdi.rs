//! The GDI drawing state shared by the WMF and EMF players: device context,
//! object table, mapping modes, path brackets, clipping, and the drawing
//! calls in logical coordinates.

use std::sync::Arc;

use crate::canvas::{Paint, Path, Pt, Seg, Stroke, Svg, Text, Xf, gdi_arc};
use crate::dib::Rgba;

#[derive(Clone, Debug)]
pub(crate) struct Pen {
    pub style: u32,
    /// Logical units; 0 is one device pixel.
    pub width: f64,
    pub color: [u8; 3],
    /// Geometric (scales with the map) rather than cosmetic.
    pub geometric: bool,
    pub user_dash: Vec<f64>,
}

impl Pen {
    pub fn solid(color: [u8; 3]) -> Self {
        Pen {
            style: 0,
            width: 0.0,
            color,
            geometric: false,
            user_dash: Vec::new(),
        }
    }

    pub fn null() -> Self {
        Pen {
            style: 5,
            ..Pen::solid([0, 0, 0])
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) enum Brush {
    Null,
    Solid([u8; 3]),
    Hatch(u32, [u8; 3]),
    /// A PNG tile and its size in pixels.
    Tile(Arc<Vec<u8>>, u32, u32),
}

#[derive(Clone, Debug)]
pub(crate) struct Font {
    pub height: f64,
    pub width: f64,
    pub escapement: f64,
    pub weight: i32,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
    pub charset: u8,
    pub face: String,
}

impl Default for Font {
    fn default() -> Self {
        Font {
            height: 0.0,
            width: 0.0,
            escapement: 0.0,
            weight: 400,
            italic: false,
            underline: false,
            strike: false,
            charset: 0,
            face: "Arial".into(),
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) enum Obj {
    Pen(Pen),
    Brush(Brush),
    Font(Font),
    /// Palettes, regions and the like: they hold a slot, nothing more.
    Other,
}

#[derive(Clone, Debug)]
pub(crate) struct Dc {
    pub pen: Pen,
    pub brush: Brush,
    pub font: Font,
    pub text_color: [u8; 3],
    pub bk_color: [u8; 3],
    /// 1 transparent, 2 opaque.
    pub bk_mode: u32,
    pub text_align: u32,
    /// 1 alternate (even-odd), 2 winding.
    pub fill_mode: u32,
    pub map_mode: u32,
    pub win_org: Pt,
    pub win_ext: Option<Pt>,
    pub vp_org: Pt,
    pub vp_ext: Option<Pt>,
    pub world: Xf,
    pub cur: Pt,
    pub clockwise: bool,
    pub clip: Option<String>,
    pub miter: f64,
}

impl Default for Dc {
    fn default() -> Self {
        Dc {
            pen: Pen::solid([0, 0, 0]),
            brush: Brush::Solid([255, 255, 255]),
            font: Font::default(),
            text_color: [0, 0, 0],
            bk_color: [255, 255, 255],
            bk_mode: 2,
            text_align: 0,
            fill_mode: 1,
            map_mode: 1,
            win_org: (0.0, 0.0),
            win_ext: None,
            vp_org: (0.0, 0.0),
            vp_ext: None,
            world: Xf::ID,
            cur: (0.0, 0.0),
            clockwise: false,
            clip: None,
            miter: 10.0,
        }
    }
}

/// How logical coordinates reach the device.
pub(crate) enum Mapping {
    /// WMF: the current window is stretched over the picture's frame
    /// (device units are the frame's logical units).
    Wmf { frame: [f64; 4] },
    /// EMF: GDI's map modes, with the reference device's pixels per mm.
    Emf { px_per_mm: Pt },
}

pub(crate) struct Gdi {
    pub svg: Svg,
    pub dc: Dc,
    saved: Vec<Dc>,
    pub objects: Vec<Option<Obj>>,
    pub mapping: Mapping,
    /// A path bracket being recorded (BeginPath … EndPath).
    bracket: Option<Path>,
    /// The path EndPath finished, for FillPath and friends.
    pub done_path: Option<Path>,
    /// Lines from LineTo waiting to be drawn as one polyline.
    pending: Option<Path>,
    /// A view rectangle far beyond the picture (for exclusion clips).
    pub huge: [f64; 4],
}

pub(crate) fn stock(index: u32) -> Option<Obj> {
    Some(match index {
        0 => Obj::Brush(Brush::Solid([255, 255, 255])),
        1 => Obj::Brush(Brush::Solid([192, 192, 192])),
        2 => Obj::Brush(Brush::Solid([128, 128, 128])),
        3 => Obj::Brush(Brush::Solid([64, 64, 64])),
        4 => Obj::Brush(Brush::Solid([0, 0, 0])),
        5 => Obj::Brush(Brush::Null),
        6 => Obj::Pen(Pen::solid([255, 255, 255])),
        7 => Obj::Pen(Pen::solid([0, 0, 0])),
        8 => Obj::Pen(Pen::null()),
        10..=17 => Obj::Font(Font::default()),
        18 => Obj::Brush(Brush::Solid([255, 255, 255])),
        19 => Obj::Pen(Pen::solid([0, 0, 0])),
        _ => return None,
    })
}

impl Gdi {
    pub fn new(mapping: Mapping, unit: f64) -> Self {
        Gdi {
            svg: Svg::new(unit),
            dc: Dc::default(),
            saved: Vec::new(),
            objects: Vec::new(),
            mapping,
            bracket: None,
            done_path: None,
            pending: None,
            huge: [-1e7, -1e7, 2e7, 2e7],
        }
    }

    // ---- mapping -------------------------------------------------------

    /// Logical (page) → device.
    pub fn page(&self) -> Xf {
        let dc = &self.dc;
        match &self.mapping {
            Mapping::Wmf { frame } => {
                let [fx, fy, fw, fh] = *frame;
                let ext = dc.win_ext.unwrap_or((fw, fh));
                let sx = if ext.0 != 0.0 { fw / ext.0 } else { 1.0 };
                let sy = if ext.1 != 0.0 { fh / ext.1 } else { 1.0 };
                let (sx, sy) = if dc.map_mode == 7 {
                    let s = sx.abs().min(sy.abs());
                    (s * sx.signum(), s * sy.signum())
                } else {
                    (sx, sy)
                };
                Xf::translate(-dc.win_org.0, -dc.win_org.1)
                    .then(&Xf::scaling(sx, sy))
                    .then(&Xf::translate(fx, fy))
            }
            Mapping::Emf { px_per_mm } => {
                let metric = |mm: f64| (mm * px_per_mm.0, -mm * px_per_mm.1);
                let (sx, sy) = match dc.map_mode {
                    2 => metric(0.1),
                    3 => metric(0.01),
                    4 => metric(0.254),
                    5 => metric(0.0254),
                    6 => metric(25.4 / 1440.0),
                    7 | 8 => {
                        let we = dc.win_ext.unwrap_or((1.0, 1.0));
                        let ve = dc.vp_ext.unwrap_or((1.0, 1.0));
                        let sx = if we.0 != 0.0 { ve.0 / we.0 } else { 1.0 };
                        let sy = if we.1 != 0.0 { ve.1 / we.1 } else { 1.0 };
                        if dc.map_mode == 7 {
                            let s = sx.abs().min(sy.abs());
                            (s * sx.signum(), s * sy.signum())
                        } else {
                            (sx, sy)
                        }
                    }
                    _ => return Xf::ID,
                };
                Xf::translate(-dc.win_org.0, -dc.win_org.1)
                    .then(&Xf::scaling(sx, sy))
                    .then(&Xf::translate(dc.vp_org.0, dc.vp_org.1))
            }
        }
    }

    /// World → device.
    pub fn xf(&self) -> Xf {
        self.dc.world.then(&self.page())
    }

    pub fn dev(&self, p: Pt) -> Pt {
        self.xf().apply(p)
    }

    // ---- state ---------------------------------------------------------

    pub fn save(&mut self) {
        self.saved.push(self.dc.clone());
    }

    /// RestoreDC: `n` < 0 is relative to the top of the stack.
    pub fn restore(&mut self, n: i32) {
        let keep_cur = self.dc.cur;
        let target = if n < 0 {
            self.saved.len() as i64 + n as i64
        } else {
            n as i64 - 1
        };
        if target < 0 || target as usize >= self.saved.len() {
            return;
        }
        self.saved.truncate(target as usize + 1);
        if let Some(dc) = self.saved.pop() {
            self.dc = dc;
        }
        self.dc.cur = keep_cur;
        self.svg.clip = self.dc.clip.clone();
    }

    /// WMF puts a new object in the lowest free slot.
    pub fn add_lowest(&mut self, obj: Obj) {
        match self.objects.iter().position(Option::is_none) {
            Some(i) => self.objects[i] = Some(obj),
            None => self.objects.push(Some(obj)),
        }
    }

    pub fn put(&mut self, index: usize, obj: Obj) {
        if index > 65_535 {
            return;
        }
        if self.objects.len() <= index {
            self.objects.resize(index + 1, None);
        }
        self.objects[index] = Some(obj);
    }

    pub fn delete(&mut self, index: usize) {
        if let Some(slot) = self.objects.get_mut(index) {
            *slot = None;
        }
    }

    pub fn select(&mut self, obj: Option<Obj>) {
        match obj {
            Some(Obj::Pen(p)) => self.dc.pen = p,
            Some(Obj::Brush(b)) => self.dc.brush = b,
            Some(Obj::Font(f)) => self.dc.font = f,
            _ => {}
        }
    }

    pub fn select_index(&mut self, index: usize) {
        let obj = self.objects.get(index).cloned().flatten();
        self.select(obj);
    }

    // ---- paint ---------------------------------------------------------

    fn fill_paint(&self) -> Option<Paint> {
        match &self.dc.brush {
            Brush::Null => None,
            Brush::Solid(c) => Some(Paint::Color(*c, 1.0)),
            Brush::Hatch(s, c) => Some(Paint::Hatch(
                *s,
                *c,
                (self.dc.bk_mode == 2).then_some(self.dc.bk_color),
            )),
            Brush::Tile(png, w, h) => Some(Paint::Tile(
                png.clone(),
                *w as f64 * self.svg.unit,
                *h as f64 * self.svg.unit,
            )),
        }
    }

    fn stroke(&self) -> Option<Stroke> {
        let pen = &self.dc.pen;
        let style = pen.style & 0x0F;
        if style == 5 {
            return None;
        }
        let unit = self.svg.unit;
        // Cosmetic pens (and width 0) are one pixel wide.
        let width = if pen.geometric && pen.width > 0.0 {
            (pen.width * self.xf().scale()).max(unit * 0.5)
        } else {
            unit
        };
        let b = width.max(unit);
        let dash: Vec<f64> = match style {
            1 => vec![6.0 * b, 2.0 * b],
            2 => vec![b, b],
            3 => vec![6.0 * b, 2.0 * b, b, 2.0 * b],
            4 => vec![6.0 * b, 2.0 * b, b, 2.0 * b, b, 2.0 * b],
            7 => pen
                .user_dash
                .iter()
                .map(|d| {
                    d * if pen.geometric {
                        self.xf().scale()
                    } else {
                        unit
                    }
                })
                .collect(),
            _ => Vec::new(),
        };
        let dash = if dash.iter().all(|d| *d <= 0.0) {
            Vec::new()
        } else {
            dash
        };
        let cap = match pen.style & 0x0F00 {
            0x100 => "square",
            0x200 => "butt",
            _ if !dash.is_empty() => "butt",
            _ => "round",
        };
        let join = match pen.style & 0xF000 {
            0x1000 => "bevel",
            0x2000 => "miter",
            _ => "round",
        };
        Some(Stroke {
            color: pen.color,
            alpha: 1.0,
            width,
            dash,
            cap,
            join,
            miter: self.dc.miter,
        })
    }

    /// Draw a figure (device path) with the brush and/or pen, or add it to
    /// the path bracket.
    pub fn shape(&mut self, p: Path, fill: bool, stroke: bool) {
        if let Some(b) = &mut self.bracket {
            b.append(&p);
            return;
        }
        let f = if fill { self.fill_paint() } else { None };
        let s = if stroke { self.stroke() } else { None };
        let evenodd = self.dc.fill_mode == 1;
        self.svg.path(&p, f.as_ref(), evenodd, s.as_ref());
    }

    pub fn fill_with(&mut self, p: &Path, paint: &Paint) {
        self.svg.path(p, Some(paint), self.dc.fill_mode == 1, None);
    }

    // ---- lines ---------------------------------------------------------

    pub fn move_to(&mut self, p: Pt) {
        self.dc.cur = p;
        let d = self.dev(p);
        if let Some(b) = &mut self.bracket {
            b.move_to(d);
        } else if let Some(pending) = &mut self.pending {
            pending.move_to(d);
        }
    }

    pub fn line_to(&mut self, p: Pt) {
        let from = self.dev(self.dc.cur);
        let to = self.dev(p);
        self.dc.cur = p;
        if let Some(b) = &mut self.bracket {
            if matches!(b.segs.last(), None | Some(Seg::Z)) {
                b.move_to(from);
            }
            b.line_to(to);
            return;
        }
        let pending = self.pending.get_or_insert_with(Path::default);
        match pending.segs.last() {
            Some(Seg::M(q) | Seg::L(q)) if *q == from => {}
            Some(Seg::C(_, _, q)) if *q == from => {}
            _ => pending.move_to(from),
        }
        pending.line_to(to);
    }

    /// Draw lines waiting from LineTo (call before anything else changes).
    pub fn flush(&mut self) {
        if let Some(p) = self.pending.take() {
            let s = self.stroke();
            self.svg.path(&p, None, false, s.as_ref());
        }
    }

    /// Polyline/PolylineTo/PolyBezierTo in a bracket continue the figure;
    /// outside one they draw with the pen.
    pub fn open_figure(&mut self, p: Path, continues: bool) {
        if let Some(b) = &mut self.bracket {
            let mut segs = p.segs;
            if continues
                && !matches!(b.segs.last(), None | Some(Seg::Z))
                && let Some(Seg::M(_)) = segs.first()
            {
                segs.remove(0);
            }
            b.segs.extend(segs);
            return;
        }
        let s = self.stroke();
        self.svg.path(&p, None, false, s.as_ref());
    }

    pub fn arc(&mut self, rect: (f64, f64, f64, f64), s: Pt, e: Pt, kind: ArcKind) {
        let m = self.xf();
        let mut p = Path::default();
        let clockwise = self.dc.clockwise;
        match kind {
            ArcKind::Arc => {
                gdi_arc(&mut p, rect, s, e, clockwise, &m, true);
                self.open_figure(p, false);
            }
            ArcKind::ArcTo => {
                let from = m.apply(self.dc.cur);
                p.move_to(from);
                gdi_arc(&mut p, rect, s, e, clockwise, &m, false);
                if let Some(Seg::C(_, _, last)) = p.segs.last() {
                    let inv = invert(&m);
                    if let Some(inv) = inv {
                        self.dc.cur = inv.apply(*last);
                    }
                }
                self.open_figure(p, true);
            }
            ArcKind::Chord => {
                gdi_arc(&mut p, rect, s, e, clockwise, &m, true);
                p.close();
                self.shape(p, true, true);
            }
            ArcKind::Pie => {
                let (l, t, r, b) = rect;
                p.move_to(m.apply(((l + r) / 2.0, (t + b) / 2.0)));
                gdi_arc(&mut p, rect, s, e, clockwise, &m, false);
                p.close();
                self.shape(p, true, true);
            }
        }
    }

    // ---- paths ---------------------------------------------------------

    pub fn begin_path(&mut self) {
        self.flush();
        self.bracket = Some(Path::default());
    }

    pub fn end_path(&mut self) {
        self.done_path = self.bracket.take();
    }

    pub fn abort_path(&mut self) {
        self.bracket = None;
        self.done_path = None;
    }

    pub fn close_figure(&mut self) {
        if let Some(b) = &mut self.bracket {
            b.close();
        }
    }

    pub fn in_bracket(&self) -> bool {
        self.bracket.is_some()
    }

    pub fn draw_done_path(&mut self, fill: bool, stroke: bool) {
        if let Some(p) = self.done_path.take() {
            let mut p = p;
            if fill {
                // Filling closes every open figure.
                let mut closed = Path::default();
                for s in p.segs {
                    if matches!(s, Seg::M(_)) && !matches!(closed.segs.last(), None | Some(Seg::Z))
                    {
                        closed.close();
                    }
                    closed.segs.push(s);
                }
                closed.close();
                p = closed;
            }
            let f = if fill { self.fill_paint() } else { None };
            let s = if stroke { self.stroke() } else { None };
            self.svg
                .path(&p, f.as_ref(), self.dc.fill_mode == 1, s.as_ref());
        }
    }

    // ---- clipping ------------------------------------------------------

    fn set_clip(&mut self, id: Option<String>) {
        self.dc.clip = id.clone();
        self.svg.clip = id;
    }

    pub fn clip_reset(&mut self) {
        self.set_clip(None);
    }

    /// `rgn_and`: intersect with the current clip, else replace it.
    pub fn clip_to(&mut self, p: &Path, evenodd: bool, rgn_and: bool) {
        let id = self.svg.clip_path(p, evenodd, rgn_and);
        self.set_clip(Some(id));
    }

    pub fn intersect_clip_rect(&mut self, r: (f64, f64, f64, f64)) {
        let p = Path::rect(r, &self.xf());
        self.clip_to(&p, false, true);
    }

    pub fn exclude_clip_rect(&mut self, r: (f64, f64, f64, f64)) {
        let [x, y, w, h] = self.huge;
        let mut p = Path::rect((x, y, x + w, y + h), &Xf::ID);
        p.append(&Path::rect(r, &self.xf()));
        self.clip_to(&p, true, true);
    }

    pub fn select_clip_path(&mut self, mode: u32) {
        if let Some(p) = self.done_path.take() {
            let evenodd = self.dc.fill_mode == 1;
            self.clip_to(&p, evenodd, mode == 1);
        }
    }

    // ---- text ----------------------------------------------------------

    /// ExtTextOut / TextOut at logical `at`; `dx` are logical advances.
    pub fn text(
        &mut self,
        at: Pt,
        text: &str,
        dx: Option<Vec<f64>>,
        opaque_rect: Option<(f64, f64, f64, f64)>,
    ) {
        if self.in_bracket() {
            return;
        }
        let m = self.xf();
        if let Some(r) = opaque_rect {
            let p = Path::rect(r, &m);
            let c = self.dc.bk_color;
            self.svg.path(&p, Some(&Paint::Color(c, 1.0)), false, None);
        }
        if text.is_empty() {
            return;
        }
        let align = self.dc.text_align;
        let at = if align & 1 != 0 { self.dc.cur } else { at };
        let font = self.dc.font.clone();
        let yscale = m.y_len();
        let xscale = m.x_len();
        let em = if font.height < 0.0 {
            -font.height * yscale
        } else if font.height > 0.0 {
            font.height * yscale / 1.15
        } else {
            12.0 * self.svg.unit
        };
        let (ascent, descent) = (0.905 * em, 0.212 * em);
        let n = text.chars().count();
        let advances: Option<Vec<f64>> =
            dx.map(|d| d.into_iter().take(n).map(|v| v * xscale).collect());
        let total = match &advances {
            Some(a) if a.len() == n => a.iter().sum::<f64>(),
            _ => 0.55 * em * n as f64,
        };
        let have_dx = matches!(&advances, Some(a) if a.len() == n);
        let p0 = m.apply(at);
        let world_turn = if m.0[0] >= 0.0 && m.det() > 0.0 {
            m.0[1].atan2(m.0[0]).to_degrees()
        } else {
            0.0
        };
        let rotate = -font.escapement / 10.0 + world_turn;
        let baseline = match align & 24 {
            24 => 0.0,
            8 => -descent,
            _ => ascent,
        };
        let horiz = align & 6;
        let (start_x, anchor) = match (horiz, have_dx) {
            (6, true) => (p0.0 - total / 2.0, "start"),
            (2, true) => (p0.0 - total, "start"),
            (6, false) => (p0.0, "middle"),
            (2, false) => (p0.0, "end"),
            _ => (p0.0, "start"),
        };
        let xs = if have_dx {
            let a = advances.as_ref().unwrap_or(&Vec::new()).clone();
            let mut x = start_x;
            let mut xs = Vec::with_capacity(n);
            for v in a {
                xs.push(x);
                x += v;
            }
            Some(xs)
        } else {
            None
        };
        if self.dc.bk_mode == 2 && opaque_rect.is_none() {
            let left = match anchor {
                "middle" => start_x - total / 2.0,
                "end" => start_x - total,
                _ => start_x,
            };
            let top = p0.1 + baseline - ascent;
            let mut p = Path::default();
            p.move_to((left, top));
            p.line_to((left + total, top));
            p.line_to((left + total, top + ascent + descent));
            p.line_to((left, top + ascent + descent));
            p.close();
            let p = turn_path(&p, rotate, p0);
            let c = self.dc.bk_color;
            self.svg.path(&p, Some(&Paint::Color(c, 1.0)), false, None);
        }
        let shown = symbol_text(text, &font);
        self.svg.text(&Text {
            pos: (start_x, p0.1 + baseline),
            xs,
            anchor,
            rotate,
            pivot: p0,
            size: em,
            family: &font.face,
            weight: font.weight,
            italic: font.italic,
            underline: font.underline,
            strike: font.strike,
            color: self.dc.text_color,
            alpha: 1.0,
            text: &shown,
            // lfWidth is the average character width (Arial's is 0.44 em).
            stretch: if font.width > 0.0 && em > 0.0 {
                (font.width * xscale / (0.44 * em)).clamp(0.3, 3.0)
            } else {
                1.0
            },
        });
        if align & 1 != 0 {
            let adv = total / xscale.max(1e-9);
            self.dc.cur = (at.0 + adv, at.1);
        }
    }

    // ---- pictures ------------------------------------------------------

    /// A bitmap stretched over the logical rectangle `dest` (x, y, w, h).
    pub fn bitmap(&mut self, dest: (f64, f64, f64, f64), img: &Rgba, rop: u32) {
        if self.in_bracket() {
            return;
        }
        let m = self.xf();
        let (x, y, w, h) = dest;
        let p0 = m.apply((x, y));
        let px = m.apply((x + w, y));
        let py = m.apply((x, y + h));
        let unit = Xf([
            px.0 - p0.0,
            px.1 - p0.1,
            py.0 - p0.0,
            py.1 - p0.1,
            p0.0,
            p0.1,
        ]);
        let mut img_owned;
        let mut img = img;
        let blend = match rop {
            0x0088_00C6 => Some("multiply"),
            0x00EE_0086 => Some("lighten"),
            0x0066_0046 => Some("difference"),
            0x0033_0008 => {
                img_owned = Rgba {
                    w: img.w,
                    h: img.h,
                    px: img.px.clone(),
                };
                for p in &mut img_owned.px {
                    p[0] = 255 - p[0];
                    p[1] = 255 - p[1];
                    p[2] = 255 - p[2];
                }
                img = &img_owned;
                None
            }
            _ => None,
        };
        // A 1×1 bitmap is a fill (a common way to paint a background).
        if img.w == 1 && img.h == 1 {
            let c = img.px[0];
            let p = Path::rect((x, y, x + w, y + h), &m);
            self.svg.path(
                &p,
                Some(&Paint::Color([c[0], c[1], c[2]], c[3] as f64 / 255.0)),
                false,
                None,
            );
            return;
        }
        if let Some(png) = img.png() {
            self.svg.image(&png, &unit, blend, 1.0);
        }
    }

    /// A PNG/JPEG/GIF picture stretched over the logical rectangle `dest`.
    pub fn picture_bytes(&mut self, bytes: &[u8], dest: (f64, f64, f64, f64)) {
        if self.in_bracket() {
            return;
        }
        let m = self.xf();
        let (x, y, w, h) = dest;
        let p0 = m.apply((x, y));
        let px = m.apply((x + w, y));
        let py = m.apply((x, y + h));
        let unit = Xf([
            px.0 - p0.0,
            px.1 - p0.1,
            py.0 - p0.0,
            py.1 - p0.1,
            p0.0,
            p0.1,
        ]);
        self.svg.image(bytes, &unit, None, 1.0);
    }

    /// A raster op without a source (PatBlt and friends) over `dest`.
    pub fn pat_blt(&mut self, dest: (f64, f64, f64, f64), rop: u32) {
        let (x, y, w, h) = dest;
        let p = Path::rect((x, y, x + w, y + h), &self.xf());
        match rop {
            0x0000_0042 => self.fill_with(&p, &Paint::Color([0, 0, 0], 1.0)),
            0x00FF_0062 => self.fill_with(&p, &Paint::Color([255, 255, 255], 1.0)),
            0x00F0_0021 | 0x00A0_00C9 => {
                if let Some(f) = self.fill_paint() {
                    self.svg.path(&p, Some(&f), false, None);
                }
            }
            _ => {}
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) enum ArcKind {
    Arc,
    ArcTo,
    Chord,
    Pie,
}

pub(crate) fn invert(m: &Xf) -> Option<Xf> {
    let [a, b, c, d, e, f] = m.0;
    let det = a * d - b * c;
    if det.abs() < 1e-300 {
        return None;
    }
    let (ia, ib, ic, id) = (d / det, -b / det, -c / det, a / det);
    Some(Xf([ia, ib, ic, id, -(e * ia + f * ic), -(e * ib + f * id)]))
}

fn turn_path(p: &Path, deg: f64, pivot: Pt) -> Path {
    if deg.abs() < 1e-9 {
        return p.clone();
    }
    let (s, c) = deg.to_radians().sin_cos();
    let m = Xf::translate(-pivot.0, -pivot.1)
        .then(&Xf([c, s, -s, c, 0.0, 0.0]))
        .then(&Xf::translate(pivot.0, pivot.1));
    let segs = p
        .segs
        .iter()
        .map(|s| match *s {
            Seg::M(a) => Seg::M(m.apply(a)),
            Seg::L(a) => Seg::L(m.apply(a)),
            Seg::C(a, b, c) => Seg::C(m.apply(a), m.apply(b), m.apply(c)),
            Seg::Z => Seg::Z,
        })
        .collect();
    Path { segs }
}

/// Text in a symbol font: Office writes Symbol's characters in the private
/// use area (U+F020…) or as their 8-bit codes; Symbol's letters are shown
/// as the Greek letters they stand for.
fn symbol_text(text: &str, font: &Font) -> String {
    let symbol = font.face.eq_ignore_ascii_case("symbol");
    if font.charset != 2 && !symbol {
        return text.to_owned();
    }
    text.chars()
        .map(|c| {
            let code = c as u32;
            let code = if (0xF020..=0xF0FF).contains(&code) {
                code - 0xF000
            } else {
                code
            };
            if !symbol {
                return char::from_u32(code).unwrap_or(c);
            }
            const GREEK_UPPER: &str = "ΑΒΧΔΕΦΓΗΙϑΚΛΜΝΟΠΘΡΣΤΥςΩΞΨΖ";
            const GREEK_LOWER: &str = "αβχδεφγηιϕκλμνοπθρστυϖωξψζ";
            match code {
                0x41..=0x5A => GREEK_UPPER.chars().nth((code - 0x41) as usize).unwrap_or(c),
                0x61..=0x7A => GREEK_LOWER.chars().nth((code - 0x61) as usize).unwrap_or(c),
                0xB7 => '•',
                0xB0 => '°',
                0xB1 => '±',
                0xB4 => '×',
                0xB8 => '÷',
                0xA3 => '≤',
                0xB3 => '≥',
                0xB9 => '≠',
                0xA5 => '∞',
                0xD6 => '√',
                0xE5 => '∑',
                0xF2 => '∫',
                0xAE => '→',
                0xAC => '←',
                0xAD => '↑',
                0xAF => '↓',
                _ => char::from_u32(code).unwrap_or(c),
            }
        })
        .collect()
}
