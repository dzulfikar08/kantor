//! EMF+ ([MS-EMFPLUS]) records: the basics GDI+ writes (solid, hatch and
//! linear-gradient brushes, pens, paths, shapes, pictures, text, world
//! transforms, clipping, saves and containers).
//!
//! "EMF+ dual" files also carry the same drawing as plain EMF records; for
//! them the EMF records are drawn and these are skipped. "EMF+ only" files
//! are drawn from these records (plus EMF records after a GetDC).

use std::collections::HashMap;

use crate::canvas::{Paint, Path, Pt, Stroke, Text, Xf, arc_points};
use crate::gdi::Gdi;
use crate::rd::{f32_at, i16_at, i32_at, u8_at, u16_at, u32_at, utf16};

#[derive(Clone, Debug)]
enum PBrush {
    Solid([u8; 4]),
    Hatch(u32, [u8; 4], [u8; 4]),
    /// Rectangle (world) and its two colours, left to right.
    Linear([f64; 4], [u8; 4], [u8; 4]),
}

#[derive(Clone, Debug)]
struct PPen {
    width: f64,
    unit: u32,
    brush: PBrush,
    dash: Vec<f64>,
    cap: &'static str,
    join: &'static str,
    miter: f64,
}

#[derive(Clone, Debug)]
enum PObj {
    Brush(PBrush),
    Pen(PPen),
    /// World-space path.
    Path(Vec<(Pt, u8)>),
    Font {
        em: f64,
        unit: u32,
        style: u32,
        family: String,
    },
    /// PNG/JPEG/GIF bytes and the picture's size in pixels.
    Image(Vec<u8>, f64, f64),
    Format {
        align: u32,
        line_align: u32,
    },
    Other,
}

struct Saved {
    world: Xf,
    unit: u32,
    scale: f64,
    clip: Option<String>,
}

pub(crate) struct Plus {
    /// An EMF+ only file: EMF records are skipped (except after GetDC).
    pub only: bool,
    pub gdi_enabled: bool,
    seen: bool,
    dpi: Pt,
    /// EMF device units per pixel at the EMF+ DPI.
    ppm: Pt,
    world: Xf,
    unit: u32,
    scale: f64,
    objects: HashMap<u8, PObj>,
    saves: HashMap<u32, Saved>,
    /// An object split over several records: id, total size, bytes so far.
    partial: Option<(u8, u16, usize, Vec<u8>)>,
}

fn argb(v: u32) -> [u8; 4] {
    [(v >> 16) as u8, (v >> 8) as u8, v as u8, (v >> 24) as u8]
}

fn rgb_a(c: [u8; 4]) -> ([u8; 3], f64) {
    ([c[0], c[1], c[2]], c[3] as f64 / 255.0)
}

impl Plus {
    pub fn new(ppm: Pt) -> Self {
        Plus {
            only: false,
            gdi_enabled: true,
            seen: false,
            dpi: (96.0, 96.0),
            ppm,
            world: Xf::ID,
            unit: 2,
            scale: 1.0,
            objects: HashMap::new(),
            saves: HashMap::new(),
            partial: None,
        }
    }

    /// Pixels per `unit` at the file's DPI.
    fn unit_px(&self, unit: u32) -> f64 {
        match unit {
            3 => self.dpi.0 / 72.0,
            4 => self.dpi.0,
            5 => self.dpi.0 / 300.0,
            6 => self.dpi.0 / 25.4,
            _ => 1.0,
        }
    }

    /// World → EMF device units.
    fn xf(&self) -> Xf {
        let s = self.unit_px(self.unit) * self.scale;
        let dx = self.ppm.0 * 25.4 / self.dpi.0;
        let dy = self.ppm.1 * 25.4 / self.dpi.1;
        self.world.then(&Xf::scaling(s * dx, s * dy))
    }

    pub fn records(&mut self, g: &mut Gdi, data: &[u8]) {
        let mut at = 0;
        while at + 12 <= data.len() {
            let ty = u16_at(data, at);
            let flags = u16_at(data, at + 2);
            let size = u32_at(data, at + 4) as usize;
            let dsize = u32_at(data, at + 8) as usize;
            if size < 12 {
                break;
            }
            let d = data
                .get(at + 12..(at + 12 + dsize).min(data.len()))
                .unwrap_or(&[]);
            self.record(g, ty, flags, d);
            at += size;
        }
    }

    fn record(&mut self, g: &mut Gdi, ty: u16, flags: u16, d: &[u8]) {
        if ty == 0x4001 {
            self.seen = true;
            self.only = flags & 1 == 0;
            self.gdi_enabled = !self.only;
            let (dx, dy) = (u32_at(d, 8) as f64, u32_at(d, 12) as f64);
            if dx > 0.0 && dy > 0.0 {
                self.dpi = (dx, dy);
            }
            return;
        }
        if !self.seen || !self.only {
            // Dual files: the EMF records draw the same thing.
            return;
        }
        if ty == 0x4004 {
            self.gdi_enabled = true;
            return;
        }
        self.gdi_enabled = false;
        let compressed = flags & 0x4000 != 0;
        let relative = flags & 0x0800 != 0;
        let id = (flags & 0xFF) as u8;
        match ty {
            0x4008 => self.object(flags, d),
            0x4009 => {
                let (c, a) = rgb_a(argb(u32_at(d, 0)));
                let [x, y, w, h] = g.huge;
                let p = Path::rect((x, y, x + w, y + h), &Xf::ID);
                g.svg.path(&p, Some(&Paint::Color(c, a)), false, None);
            }
            0x400A => {
                let brush = self.brush_of(flags, u32_at(d, 0));
                let n = u32_at(d, 4) as usize;
                let rects = rects(d, 8, n, compressed);
                let m = self.xf();
                let mut path = Path::default();
                for [x, y, w, h] in rects {
                    path.append(&Path::rect((x, y, x + w, y + h), &m));
                }
                self.fill(g, &path, brush.as_ref(), false);
            }
            0x400B => {
                let n = u32_at(d, 0) as usize;
                let m = self.xf();
                let mut path = Path::default();
                for [x, y, w, h] in rects(d, 4, n, compressed) {
                    path.append(&Path::rect((x, y, x + w, y + h), &m));
                }
                self.stroke(g, &path, id);
            }
            0x400C => {
                let brush = self.brush_of(flags, u32_at(d, 0));
                let n = u32_at(d, 4) as usize;
                let pts = points(d, 8, n, compressed, relative);
                let path = Path::poly(&pts, &self.xf(), true);
                self.fill(g, &path, brush.as_ref(), false);
            }
            0x400D => {
                let n = u32_at(d, 0) as usize;
                let pts = points(d, 4, n, compressed, relative);
                let path = Path::poly(&pts, &self.xf(), flags & 0x2000 != 0);
                self.stroke(g, &path, id);
            }
            0x400E => {
                let brush = self.brush_of(flags, u32_at(d, 0));
                let [x, y, w, h] = rect(d, 4, compressed);
                let path = Path::ellipse((x, y, x + w, y + h), &self.xf());
                self.fill(g, &path, brush.as_ref(), false);
            }
            0x400F => {
                let [x, y, w, h] = rect(d, 0, compressed);
                let path = Path::ellipse((x, y, x + w, y + h), &self.xf());
                self.stroke(g, &path, id);
            }
            0x4010 => {
                let brush = self.brush_of(flags, u32_at(d, 0));
                let path = self.pie(f32_at(d, 4), f32_at(d, 8), rect(d, 12, compressed), true);
                self.fill(g, &path, brush.as_ref(), false);
            }
            0x4011 | 0x4012 => {
                let path = self.pie(
                    f32_at(d, 0),
                    f32_at(d, 4),
                    rect(d, 8, compressed),
                    ty == 0x4011,
                );
                self.stroke(g, &path, id);
            }
            0x4014 => {
                let brush = self.brush_of(flags, u32_at(d, 0));
                if let Some(PObj::Path(pp)) = self.objects.get(&id) {
                    let path = path_of(pp, &self.xf(), true);
                    self.fill(g, &path, brush.as_ref(), false);
                }
            }
            0x4015 => {
                let pen = u32_at(d, 0) as u8;
                if let Some(PObj::Path(pp)) = self.objects.get(&id) {
                    let path = path_of(pp, &self.xf(), false);
                    self.stroke(g, &path, pen);
                }
            }
            0x4016 => {
                let brush = self.brush_of(flags, u32_at(d, 0));
                let tension = f32_at(d, 4);
                let n = u32_at(d, 8) as usize;
                let pts = points(d, 12, n, compressed, relative);
                let path = cardinal(&pts, tension, true, &self.xf());
                self.fill(g, &path, brush.as_ref(), false);
            }
            0x4017 => {
                let tension = f32_at(d, 0);
                let n = u32_at(d, 4) as usize;
                let pts = points(d, 8, n, compressed, relative);
                let path = cardinal(&pts, tension, true, &self.xf());
                self.stroke(g, &path, id);
            }
            0x4018 => {
                let tension = f32_at(d, 0);
                let offset = u32_at(d, 4) as usize;
                let segs = u32_at(d, 8) as usize;
                let n = u32_at(d, 12) as usize;
                let pts = points(d, 16, n, compressed, relative);
                let end = (offset + segs + 1).min(pts.len());
                let pts = pts.get(offset.min(end)..end).unwrap_or(&[]);
                let path = cardinal(pts, tension, false, &self.xf());
                self.stroke(g, &path, id);
            }
            0x4019 => {
                let n = u32_at(d, 0) as usize;
                let pts = points(d, 4, n, compressed, relative);
                let path = Path::bezier(&pts, &self.xf(), None);
                self.stroke(g, &path, id);
            }
            0x401A => {
                let src = [f32_at(d, 8), f32_at(d, 12), f32_at(d, 16), f32_at(d, 20)];
                let [x, y, w, h] = rect(d, 24, compressed);
                self.image(g, id, src, [(x, y), (x + w, y), (x, y + h)]);
            }
            0x401B => {
                let src = [f32_at(d, 8), f32_at(d, 12), f32_at(d, 16), f32_at(d, 20)];
                let pts = points(d, 28, 3, compressed, relative);
                if pts.len() == 3 {
                    self.image(g, id, src, [pts[0], pts[1], pts[2]]);
                }
            }
            0x401C => self.draw_string(g, flags, d),
            0x4036 => self.driver_string(g, flags, d),
            0x4025 | 0x4028 => {
                let k = u32_at(d, 0);
                self.save(g, k);
            }
            0x4027 => {
                // BeginContainer: dest rect, src rect, stack index.
                let dst = [f32_at(d, 0), f32_at(d, 4), f32_at(d, 8), f32_at(d, 12)];
                let src = [f32_at(d, 16), f32_at(d, 20), f32_at(d, 24), f32_at(d, 28)];
                let k = u32_at(d, 32);
                self.save(g, k);
                if src[2] != 0.0 && src[3] != 0.0 {
                    let m = Xf::translate(-src[0], -src[1])
                        .then(&Xf::scaling(dst[2] / src[2], dst[3] / src[3]))
                        .then(&Xf::translate(dst[0], dst[1]));
                    self.world = m.then(&self.world);
                }
            }
            0x4026 | 0x4029 => {
                let k = u32_at(d, 0);
                if let Some(s) = self.saves.remove(&k) {
                    self.world = s.world;
                    self.unit = s.unit;
                    self.scale = s.scale;
                    g.dc.clip = s.clip.clone();
                    g.svg.clip = s.clip;
                }
            }
            0x402A => {
                let m = matrix(d, 0);
                if m.is_finite() {
                    self.world = m;
                }
            }
            0x402B => self.world = Xf::ID,
            0x402C => self.combine(flags, matrix(d, 0)),
            0x402D => self.combine(flags, Xf::translate(f32_at(d, 0), f32_at(d, 4))),
            0x402E => self.combine(flags, Xf::scaling(f32_at(d, 0), f32_at(d, 4))),
            0x402F => {
                let (s, c) = f32_at(d, 0).to_radians().sin_cos();
                self.combine(flags, Xf([c, s, -s, c, 0.0, 0.0]));
            }
            0x4030 => {
                self.unit = (flags & 0xFF) as u32;
                let s = f32_at(d, 0);
                self.scale = if s > 0.0 { s } else { 1.0 };
            }
            0x4031 => g.clip_reset(),
            0x4032 => {
                let [x, y, w, h] = [f32_at(d, 0), f32_at(d, 4), f32_at(d, 8), f32_at(d, 12)];
                let p = Path::rect((x, y, x + w, y + h), &self.xf());
                self.clip(g, &p, (flags >> 8) & 0xF);
            }
            0x4033 => {
                if let Some(PObj::Path(pp)) = self.objects.get(&id) {
                    let p = path_of(pp, &self.xf(), true);
                    self.clip(g, &p, (flags >> 8) & 0xF);
                }
            }
            _ => {}
        }
    }

    fn combine(&mut self, flags: u16, m: Xf) {
        if !m.is_finite() {
            return;
        }
        self.world = if flags & 0x2000 != 0 {
            self.world.then(&m)
        } else {
            m.then(&self.world)
        };
    }

    fn save(&mut self, g: &Gdi, k: u32) {
        self.saves.insert(
            k,
            Saved {
                world: self.world,
                unit: self.unit,
                scale: self.scale,
                clip: g.dc.clip.clone(),
            },
        );
    }

    fn clip(&self, g: &mut Gdi, p: &Path, mode: u16) {
        match mode {
            0 => g.clip_to(p, false, false),
            1 => g.clip_to(p, false, true),
            4 => {
                let [x, y, w, h] = g.huge;
                let mut all = Path::rect((x, y, x + w, y + h), &Xf::ID);
                all.append(p);
                g.clip_to(&all, true, true);
            }
            _ => {}
        }
    }

    fn pie(&self, start: f64, sweep: f64, [x, y, w, h]: [f64; 4], pie: bool) -> Path {
        let m = self.xf();
        let (cx, cy) = (x + w / 2.0, y + h / 2.0);
        let mut p = Path::default();
        if pie {
            p.move_to(m.apply((cx, cy)));
        }
        arc_points(
            &mut p,
            (cx, cy),
            (w / 2.0, h / 2.0),
            start.to_radians(),
            sweep.to_radians(),
            &m,
            !pie,
        );
        if pie {
            p.close();
        }
        p
    }

    fn brush_of(&self, flags: u16, v: u32) -> Option<PBrush> {
        if flags & 0x8000 != 0 {
            return Some(PBrush::Solid(argb(v)));
        }
        match self.objects.get(&(v as u8)) {
            Some(PObj::Brush(b)) => Some(b.clone()),
            _ => None,
        }
    }

    fn paint(&self, b: &PBrush) -> Paint {
        match b {
            PBrush::Solid(c) => {
                let (c, a) = rgb_a(*c);
                Paint::Color(c, a)
            }
            PBrush::Hatch(style, fg, bg) if *style < 6 => Paint::Hatch(
                *style,
                [fg[0], fg[1], fg[2]],
                (bg[3] > 0).then_some([bg[0], bg[1], bg[2]]),
            ),
            PBrush::Hatch(_, fg, bg) => {
                let mix = |i: usize| ((fg[i] as u32 + bg[i] as u32) / 2) as u8;
                Paint::Color([mix(0), mix(1), mix(2)], 1.0)
            }
            PBrush::Linear([x, y, w, h], c0, c1) => {
                let m = self.xf();
                let mid = y + h / 2.0;
                Paint::Linear(m.apply((*x, mid)), m.apply((x + w, mid)), *c0, *c1)
            }
        }
    }

    fn fill(&self, g: &mut Gdi, p: &Path, b: Option<&PBrush>, evenodd: bool) {
        if let Some(b) = b {
            let paint = self.paint(b);
            g.svg.path(p, Some(&paint), evenodd, None);
        }
    }

    fn stroke(&self, g: &mut Gdi, p: &Path, pen: u8) {
        let Some(PObj::Pen(pen)) = self.objects.get(&pen) else {
            return;
        };
        let m = self.xf();
        let width = if pen.width <= 0.0 {
            g.svg.unit
        } else if pen.unit == 0 || pen.unit == 1 || pen.unit == 2 {
            // World units (and pixels, which GDI+ also transforms).
            pen.width * m.scale()
        } else {
            pen.width * self.unit_px(pen.unit) * self.ppm.0 * 25.4 / self.dpi.0
        };
        let (color, alpha) = match &pen.brush {
            PBrush::Solid(c) | PBrush::Hatch(_, c, _) | PBrush::Linear(_, c, _) => rgb_a(*c),
        };
        let st = Stroke {
            color,
            alpha,
            width: width.max(g.svg.unit * 0.5),
            dash: pen.dash.iter().map(|d| d * width.max(g.svg.unit)).collect(),
            cap: pen.cap,
            join: pen.join,
            miter: pen.miter,
        };
        g.svg.path(p, None, false, Some(&st));
    }

    fn image(&self, g: &mut Gdi, id: u8, src: [f64; 4], [p0, p1, p2]: [Pt; 3]) {
        let Some(PObj::Image(bytes, iw, ih)) = self.objects.get(&id) else {
            return;
        };
        let m = self.xf();
        let (a, b, c) = (m.apply(p0), m.apply(p1), m.apply(p2));
        let [sx, sy, sw, sh] = src;
        if *iw > 0.0
            && *ih > 0.0
            && sw > 0.0
            && sh > 0.0
            && (sx.abs() > 0.5 || sy.abs() > 0.5 || (sw - iw).abs() > 0.5 || (sh - ih).abs() > 0.5)
        {
            // A cropped picture: draw the whole one so that the source
            // rectangle lands on the destination, clipped to it.
            let ux = ((b.0 - a.0) / sw, (b.1 - a.1) / sw);
            let uy = ((c.0 - a.0) / sh, (c.1 - a.1) / sh);
            let origin = (a.0 - ux.0 * sx - uy.0 * sy, a.1 - ux.1 * sx - uy.1 * sy);
            let unit = Xf([
                ux.0 * iw,
                ux.1 * iw,
                uy.0 * ih,
                uy.1 * ih,
                origin.0,
                origin.1,
            ]);
            let mut clip = Path::default();
            clip.move_to(a);
            clip.line_to(b);
            clip.line_to((b.0 + c.0 - a.0, b.1 + c.1 - a.1));
            clip.line_to(c);
            clip.close();
            let saved = g.svg.clip.clone();
            let id = g.svg.clip_path(&clip, false, true);
            g.svg.clip = Some(id);
            g.svg.image(bytes, &unit, None, 1.0);
            g.svg.clip = saved;
            return;
        }
        let unit = Xf([b.0 - a.0, b.1 - a.1, c.0 - a.0, c.1 - a.1, a.0, a.1]);
        g.svg.image(bytes, &unit, None, 1.0);
    }

    fn font(&self, id: u8) -> Option<(f64, u32, String)> {
        match self.objects.get(&id) {
            Some(PObj::Font {
                em,
                unit,
                style,
                family,
            }) => {
                // Font sizes in units other than world are converted to
                // world units through the page transform.
                let world_em = if *unit == 0 {
                    *em
                } else {
                    em * self.unit_px(*unit) / (self.unit_px(self.unit) * self.scale).max(1e-9)
                };
                Some((world_em, *style, family.clone()))
            }
            _ => None,
        }
    }

    fn text_paint(&self, flags: u16, v: u32) -> ([u8; 3], f64) {
        match self.brush_of(flags, v) {
            Some(PBrush::Solid(c) | PBrush::Hatch(_, c, _) | PBrush::Linear(_, c, _)) => rgb_a(c),
            None => ([0, 0, 0], 1.0),
        }
    }

    fn draw_string(&self, g: &mut Gdi, flags: u16, d: &[u8]) {
        let (color, alpha) = self.text_paint(flags, u32_at(d, 0));
        let format = u32_at(d, 4) as u8;
        let n = u32_at(d, 8) as usize;
        let [x, y, w, h] = [f32_at(d, 12), f32_at(d, 16), f32_at(d, 20), f32_at(d, 24)];
        let text = utf16(d, 28, n.min(100_000));
        let Some((em, style, family)) = self.font((flags & 0xFF) as u8) else {
            return;
        };
        let (align, line_align) = match self.objects.get(&format) {
            Some(PObj::Format { align, line_align }) => (*align, *line_align),
            _ => (0, 0),
        };
        let lines: Vec<&str> = text.split(['\n', '\r']).filter(|l| !l.is_empty()).collect();
        let lh = em * 1.15;
        let block = lh * lines.len() as f64;
        let top = match line_align {
            1 if h > 0.0 => y + (h - block) / 2.0,
            2 if h > 0.0 => y + h - block,
            _ => y,
        };
        let m = self.xf();
        let turn = m.0[1].atan2(m.0[0]).to_degrees();
        for (i, line) in lines.iter().enumerate() {
            let (lx, anchor) = match align {
                1 if w > 0.0 => (x + w / 2.0, "middle"),
                2 if w > 0.0 => (x + w, "end"),
                _ => (x, "start"),
            };
            let base = (lx, top + i as f64 * lh + 0.905 * em);
            let p = m.apply(base);
            g.svg.text(&Text {
                pos: p,
                xs: None,
                anchor,
                rotate: if m.det() > 0.0 { turn } else { 0.0 },
                pivot: p,
                size: em * m.y_len(),
                family: &family,
                weight: if style & 1 != 0 { 700 } else { 400 },
                italic: style & 2 != 0,
                underline: style & 4 != 0,
                strike: style & 8 != 0,
                color,
                alpha,
                text: line,
                stretch: 1.0,
            });
        }
    }

    fn driver_string(&self, g: &mut Gdi, flags: u16, d: &[u8]) {
        let (color, alpha) = self.text_paint(flags, u32_at(d, 0));
        let options = u32_at(d, 4);
        let n = u32_at(d, 12) as usize;
        if options & 1 == 0 || n == 0 || n > 100_000 {
            // Glyph indices without the font file can't be shown as text.
            return;
        }
        let text = utf16(d, 16, n);
        let pos_at = 16 + n * 2;
        let Some((em, style, family)) = self.font((flags & 0xFF) as u8) else {
            return;
        };
        let m = self.xf();
        let pts: Vec<Pt> = (0..n)
            .map(|i| m.apply((f32_at(d, pos_at + i * 8), f32_at(d, pos_at + i * 8 + 4))))
            .collect();
        let Some(&first) = pts.first() else {
            return;
        };
        let xs: Vec<f64> = if options & 4 != 0 {
            // RealizedAdvance: only the first position is given.
            Vec::new()
        } else {
            pts.iter().map(|p| p.0).collect()
        };
        g.svg.text(&Text {
            pos: first,
            xs: (xs.len() == text.chars().count()).then_some(xs),
            anchor: "start",
            rotate: 0.0,
            pivot: first,
            size: em * m.y_len(),
            family: &family,
            weight: if style & 1 != 0 { 700 } else { 400 },
            italic: style & 2 != 0,
            underline: style & 4 != 0,
            strike: style & 8 != 0,
            color,
            alpha,
            text: &text,
            stretch: 1.0,
        });
    }

    fn object(&mut self, flags: u16, d: &[u8]) {
        let id = (flags & 0xFF) as u8;
        let kind = (flags >> 8) & 0x7F;
        if flags & 0x8000 != 0 {
            // Continued: a total size, then this part.
            let total = u32_at(d, 0) as usize;
            let part = d.get(4..).unwrap_or(&[]);
            match &mut self.partial {
                Some((pid, pk, _, buf)) if *pid == id && *pk == kind => {
                    buf.extend_from_slice(part);
                }
                _ => {
                    self.partial = Some((id, kind, total, part.to_vec()));
                }
            }
            return;
        }
        let whole: Vec<u8>;
        let d = match self.partial.take() {
            Some((pid, pk, _, mut buf)) if pid == id && pk == kind => {
                buf.extend_from_slice(d);
                whole = buf;
                &whole[..]
            }
            _ => d,
        };
        let obj = match kind {
            1 => brush(d, 0).map(PObj::Brush),
            2 => pen(d),
            3 => path(d).map(PObj::Path),
            5 => image(d),
            6 => {
                let em = f32_at(d, 4);
                let unit = u32_at(d, 8);
                let style = u32_at(d, 12);
                let len = u32_at(d, 20) as usize;
                Some(PObj::Font {
                    em,
                    unit,
                    style,
                    family: utf16(d, 24, len.min(256)),
                })
            }
            7 => Some(PObj::Format {
                align: u32_at(d, 12),
                line_align: u32_at(d, 16),
            }),
            _ => Some(PObj::Other),
        };
        self.objects.insert(id, obj.unwrap_or(PObj::Other));
    }
}

fn brush(d: &[u8], o: usize) -> Option<PBrush> {
    let ty = u32_at(d, o + 4);
    Some(match ty {
        0 => PBrush::Solid(argb(u32_at(d, o + 8))),
        1 => PBrush::Hatch(
            u32_at(d, o + 8),
            argb(u32_at(d, o + 12)),
            argb(u32_at(d, o + 16)),
        ),
        3 => {
            // Path gradient: the centre colour mixed with the first
            // surrounding one.
            let c = argb(u32_at(d, o + 16));
            let s = if u32_at(d, o + 28) > 0 {
                argb(u32_at(d, o + 32))
            } else {
                c
            };
            let mix = |i: usize| ((c[i] as u32 + s[i] as u32) / 2) as u8;
            PBrush::Solid([mix(0), mix(1), mix(2), mix(3)])
        }
        4 => PBrush::Linear(
            [
                f32_at(d, o + 16),
                f32_at(d, o + 20),
                f32_at(d, o + 24),
                f32_at(d, o + 28),
            ],
            argb(u32_at(d, o + 32)),
            argb(u32_at(d, o + 36)),
        ),
        _ => PBrush::Solid([128, 128, 128, 255]),
    })
}

fn pen(d: &[u8]) -> Option<PObj> {
    let flags = u32_at(d, 8);
    let unit = u32_at(d, 12);
    let width = f32_at(d, 16);
    let mut o = 20;
    let mut cap = "butt";
    let mut join = "miter";
    let mut miter = 10.0;
    let mut dash = Vec::new();
    if flags & 0x01 != 0 {
        o += 24;
    }
    if flags & 0x02 != 0 {
        cap = match i32_at(d, o) {
            1 => "square",
            2 => "round",
            _ => "butt",
        };
        o += 4;
    }
    if flags & 0x04 != 0 {
        o += 4;
    }
    if flags & 0x08 != 0 {
        join = match u32_at(d, o) {
            1 => "bevel",
            2 => "round",
            _ => "miter",
        };
        o += 4;
    }
    if flags & 0x10 != 0 {
        miter = f32_at(d, o).max(1.0);
        o += 4;
    }
    if flags & 0x20 != 0 {
        dash = match i32_at(d, o) {
            1 => vec![3.0, 1.0],
            2 => vec![1.0, 1.0],
            3 => vec![3.0, 1.0, 1.0, 1.0],
            4 => vec![3.0, 1.0, 1.0, 1.0, 1.0, 1.0],
            _ => Vec::new(),
        };
        o += 4;
    }
    if flags & 0x40 != 0 {
        o += 4;
    }
    if flags & 0x80 != 0 {
        o += 4;
    }
    if flags & 0x100 != 0 {
        let n = u32_at(d, o) as usize;
        dash = (0..n.min(32)).map(|i| f32_at(d, o + 4 + i * 4)).collect();
        o += 4 + n * 4;
    }
    if flags & 0x200 != 0 {
        o += 4;
    }
    if flags & 0x400 != 0 {
        let n = u32_at(d, o) as usize;
        o += 4 + n * 4;
    }
    if flags & 0x800 != 0 {
        o += 4 + u32_at(d, o) as usize;
    }
    if flags & 0x1000 != 0 {
        o += 4 + u32_at(d, o) as usize;
    }
    let brush = brush(d, o).unwrap_or(PBrush::Solid([0, 0, 0, 255]));
    Some(PObj::Pen(PPen {
        width,
        unit,
        brush,
        dash,
        cap,
        join,
        miter,
    }))
}

fn path(d: &[u8]) -> Option<Vec<(Pt, u8)>> {
    let n = u32_at(d, 4) as usize;
    let flags = u32_at(d, 8);
    if n > 1_000_000 {
        return None;
    }
    let compressed = flags & 0x4000 != 0;
    let relative = flags & 0x0800 != 0;
    let rle = flags & 0x1000 != 0;
    let (pts, after) = points_len(d, 12, n, compressed, relative);
    let types: Vec<u8> = if rle {
        let mut t = Vec::with_capacity(n);
        let mut o = after;
        while t.len() < n && o + 1 < d.len() {
            let run = u8_at(d, o) & 0x3F;
            let ty = u8_at(d, o + 1);
            for _ in 0..run {
                t.push(ty);
            }
            o += 2;
        }
        t
    } else {
        (0..n).map(|i| u8_at(d, after + i)).collect()
    };
    Some(pts.into_iter().zip(types).collect())
}

fn path_of(pp: &[(Pt, u8)], m: &Xf, close_all: bool) -> Path {
    let mut p = Path::default();
    let mut i = 0;
    while i < pp.len() {
        let (pt, t) = pp[i];
        match t & 0x07 {
            0 => {
                if close_all && !p.segs.is_empty() {
                    p.close();
                }
                p.move_to(m.apply(pt));
            }
            3 if i + 2 < pp.len() => {
                p.cubic(m.apply(pt), m.apply(pp[i + 1].0), m.apply(pp[i + 2].0));
                i += 2;
            }
            _ => p.line_to(m.apply(pt)),
        }
        let last_t = pp[i].1;
        if last_t & 0x80 != 0 {
            p.close();
        }
        i += 1;
    }
    if close_all {
        p.close();
    }
    p
}

fn rect(d: &[u8], o: usize, compressed: bool) -> [f64; 4] {
    if compressed {
        [
            i16_at(d, o) as f64,
            i16_at(d, o + 2) as f64,
            i16_at(d, o + 4) as f64,
            i16_at(d, o + 6) as f64,
        ]
    } else {
        [
            f32_at(d, o),
            f32_at(d, o + 4),
            f32_at(d, o + 8),
            f32_at(d, o + 12),
        ]
    }
}

fn rects(d: &[u8], o: usize, n: usize, compressed: bool) -> Vec<[f64; 4]> {
    let step = if compressed { 8 } else { 16 };
    (0..n.min(d.len() / step + 1))
        .map(|i| rect(d, o + i * step, compressed))
        .collect()
}

fn points(d: &[u8], o: usize, n: usize, compressed: bool, relative: bool) -> Vec<Pt> {
    points_len(d, o, n, compressed, relative).0
}

/// Points and the offset after them.
fn points_len(d: &[u8], o: usize, n: usize, compressed: bool, relative: bool) -> (Vec<Pt>, usize) {
    let n = n.min(d.len());
    if relative {
        let mut out = Vec::with_capacity(n);
        let mut at = o;
        let (mut x, mut y) = (0.0, 0.0);
        let read = |at: &mut usize| -> f64 {
            let b0 = u8_at(d, *at);
            if b0 & 0x80 == 0 {
                *at += 1;
                (((b0 << 1) as i8) >> 1) as f64
            } else {
                let v = (((b0 & 0x7F) as u16) << 8) | u8_at(d, *at + 1) as u16;
                *at += 2;
                (((v << 1) as i16) >> 1) as f64
            }
        };
        for _ in 0..n {
            x += read(&mut at);
            y += read(&mut at);
            out.push((x, y));
        }
        return (out, at);
    }
    if compressed {
        let pts = (0..n)
            .map(|i| (i16_at(d, o + i * 4) as f64, i16_at(d, o + i * 4 + 2) as f64))
            .collect();
        (pts, o + n * 4)
    } else {
        let pts = (0..n)
            .map(|i| (f32_at(d, o + i * 8), f32_at(d, o + i * 8 + 4)))
            .collect();
        (pts, o + n * 8)
    }
}

fn matrix(d: &[u8], o: usize) -> Xf {
    Xf([
        f32_at(d, o),
        f32_at(d, o + 4),
        f32_at(d, o + 8),
        f32_at(d, o + 12),
        f32_at(d, o + 16),
        f32_at(d, o + 20),
    ])
}

/// A cardinal spline through the points (GDI+'s curves) as Béziers.
fn cardinal(pts: &[Pt], tension: f64, closed: bool, m: &Xf) -> Path {
    let n = pts.len();
    let mut p = Path::default();
    if n < 2 {
        return p;
    }
    let t = tension / 3.0;
    let at = |i: isize| -> Pt {
        if closed {
            pts[i.rem_euclid(n as isize) as usize]
        } else {
            pts[i.clamp(0, n as isize - 1) as usize]
        }
    };
    p.move_to(m.apply(pts[0]));
    let segs = if closed { n } else { n - 1 };
    for i in 0..segs as isize {
        let (p0, p1, p2, p3) = (at(i - 1), at(i), at(i + 1), at(i + 2));
        let c1 = (p1.0 + (p2.0 - p0.0) * t, p1.1 + (p2.1 - p0.1) * t);
        let c2 = (p2.0 - (p3.0 - p1.0) * t, p2.1 - (p3.1 - p1.1) * t);
        p.cubic(m.apply(c1), m.apply(c2), m.apply(p2));
    }
    if closed {
        p.close();
    }
    p
}

fn image(d: &[u8]) -> Option<PObj> {
    match u32_at(d, 4) {
        1 => {
            let (w, h) = (i32_at(d, 8), i32_at(d, 12));
            let stride = i32_at(d, 16);
            let format = u32_at(d, 20);
            let data = d.get(28..)?;
            if u32_at(d, 24) == 1 {
                // A compressed picture file (PNG, JPEG, GIF…).
                let (iw, ih) = crate::raster_size(data).unwrap_or((w as f64, h as f64));
                return if crate::raster_mime(data).is_some() {
                    Some(PObj::Image(data.to_vec(), iw, ih))
                } else {
                    crate::to_png(data, 2048).map(|png| PObj::Image(png, w as f64, h as f64))
                };
            }
            if w <= 0 || h <= 0 || (w as u64 * h as u64) > 64 * 1024 * 1024 {
                return None;
            }
            let (w, h) = (w as u32, h as u32);
            let bpp = (format >> 8) & 0xFF;
            let indexed = format & 0x0001_0000 != 0;
            let (palette, pix) = if indexed {
                let count = u32_at(data, 4) as usize;
                let pal: Vec<[u8; 4]> = (0..count.min(256))
                    .map(|i| argb(u32_at(data, 8 + i * 4)))
                    .collect();
                (pal, data.get(8 + count * 4..)?)
            } else {
                (Vec::new(), data)
            };
            let stride = if stride > 0 {
                stride as usize
            } else {
                (w as usize * bpp as usize).div_ceil(32) * 4
            };
            let mut px = Vec::with_capacity((w * h) as usize);
            let premultiplied = format == 0x000E_200B;
            let has_alpha = format & 0x0004_0000 != 0;
            for y in 0..h as usize {
                let row = pix.get(y * stride..).unwrap_or(&[]);
                for x in 0..w as usize {
                    let c = match bpp {
                        32 => {
                            let v = u32_at(row, x * 4);
                            let mut c = argb(v);
                            if !has_alpha {
                                c[3] = 255;
                            } else if premultiplied && c[3] > 0 && c[3] < 255 {
                                for k in 0..3 {
                                    c[k] = ((c[k] as u32 * 255) / c[3] as u32).min(255) as u8;
                                }
                            }
                            c
                        }
                        24 => [
                            u8_at(row, x * 3 + 2),
                            u8_at(row, x * 3 + 1),
                            u8_at(row, x * 3),
                            255,
                        ],
                        16 => {
                            let v = u16_at(row, x * 2) as u32;
                            [
                                (((v >> 11) & 31) * 255 / 31) as u8,
                                (((v >> 5) & 63) * 255 / 63) as u8,
                                ((v & 31) * 255 / 31) as u8,
                                255,
                            ]
                        }
                        8 => palette
                            .get(u8_at(row, x) as usize)
                            .copied()
                            .unwrap_or([0, 0, 0, 255]),
                        1 => {
                            let bit = (u8_at(row, x / 8) >> (7 - x % 8)) & 1;
                            palette.get(bit as usize).copied().unwrap_or([0, 0, 0, 255])
                        }
                        4 => {
                            let b = u8_at(row, x / 2);
                            let i = if x % 2 == 0 { b >> 4 } else { b & 15 };
                            palette.get(i as usize).copied().unwrap_or([0, 0, 0, 255])
                        }
                        _ => return None,
                    };
                    px.push(c);
                }
            }
            let png = crate::dib::Rgba { w, h, px }.png()?;
            Some(PObj::Image(png, w as f64, h as f64))
        }
        2 => {
            // A metafile inside: drawn as a picture.
            let size = u32_at(d, 12) as usize;
            let data = d.get(16..16 + size.min(d.len().saturating_sub(16)))?;
            let pic = crate::render(data)?;
            let png = pic.png(2048)?;
            Some(PObj::Image(png, pic.width, pic.height))
        }
        _ => None,
    }
}
