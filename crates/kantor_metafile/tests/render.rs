//! Metafiles built record by record, played and rasterised; the PNG's
//! pixels are checked where the shapes should be.

use kantor_metafile::{Kind, kind, natural_size, render, to_png};

fn pixel(png: &[u8], x: f64, y: f64) -> [u8; 4] {
    let map = resvg::tiny_skia::Pixmap::decode_png(png).expect("png");
    let (px, py) = (
        (x * map.width() as f64) as u32,
        (y * map.height() as f64) as u32,
    );
    let c = map
        .pixel(px.min(map.width() - 1), py.min(map.height() - 1))
        .unwrap();
    let c = c.demultiply();
    [c.red(), c.green(), c.blue(), c.alpha()]
}

#[test]
fn svg_to_png_keeps_the_aspect() {
    let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" width="40" height="20"><rect width="20" height="20" fill="#ff0000"/><rect x="20" width="20" height="20" fill="#0000ff"/></svg>"##;
    let png = kantor_metafile::svg_to_png(svg, 200).expect("png");
    let map = resvg::tiny_skia::Pixmap::decode_png(&png).unwrap();
    assert_eq!((map.width(), map.height()), (200, 100));
    assert!(near(pixel(&png, 0.25, 0.5), [255, 0, 0, 255]));
    assert!(near(pixel(&png, 0.75, 0.5), [0, 0, 255, 255]));
    assert!(kantor_metafile::svg_to_png(b"not svg", 100).is_none());
}

fn near(a: [u8; 4], b: [u8; 4]) -> bool {
    a.iter()
        .zip(b)
        .all(|(x, y)| (*x as i32 - y as i32).abs() <= 24)
}

// ---- WMF ------------------------------------------------------------------

struct Wmf(Vec<u8>);

impl Wmf {
    fn new() -> Self {
        Wmf(Vec::new())
    }
    fn rec(&mut self, func: u16, params: &[i16]) -> &mut Self {
        let words = 3 + params.len() as u32;
        self.0.extend_from_slice(&words.to_le_bytes());
        self.0.extend_from_slice(&func.to_le_bytes());
        for p in params {
            self.0.extend_from_slice(&p.to_le_bytes());
        }
        self
    }
    fn raw(&mut self, func: u16, bytes: &[u8]) -> &mut Self {
        let mut b = bytes.to_vec();
        if b.len() % 2 == 1 {
            b.push(0);
        }
        let words = 3 + b.len() as u32 / 2;
        self.0.extend_from_slice(&words.to_le_bytes());
        self.0.extend_from_slice(&func.to_le_bytes());
        self.0.extend_from_slice(&b);
        self
    }
    fn finish(&self, placeable: Option<(i16, i16, i16, i16, u16)>) -> Vec<u8> {
        let mut out = Vec::new();
        if let Some((l, t, r, b, inch)) = placeable {
            out.extend_from_slice(&0x9AC6_CDD7u32.to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes());
            for v in [l, t, r, b] {
                out.extend_from_slice(&v.to_le_bytes());
            }
            out.extend_from_slice(&inch.to_le_bytes());
            out.extend_from_slice(&0u32.to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes());
        }
        let body_words = (self.0.len() / 2 + 3) as u32;
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&9u16.to_le_bytes());
        out.extend_from_slice(&0x0300u16.to_le_bytes());
        out.extend_from_slice(&(9 + body_words).to_le_bytes());
        out.extend_from_slice(&4u16.to_le_bytes());
        out.extend_from_slice(&10u32.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&self.0);
        out.extend_from_slice(&3u32.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out
    }
}

fn color(r: u8, g: u8, b: u8) -> [i16; 2] {
    let v = u32::from_le_bytes([r, g, b, 0]);
    [v as u16 as i16, (v >> 16) as u16 as i16]
}

#[test]
fn wmf_placeable_shapes() {
    let mut w = Wmf::new();
    // Window 0,0 1000×1000.
    w.rec(0x020B, &[0, 0]).rec(0x020C, &[1000, 1000]);
    // Red solid brush (slot 0), null pen (slot 1).
    let [c0, c1] = color(255, 0, 0);
    w.rec(0x02FC, &[0, c0, c1, 0]);
    w.rec(0x02FA, &[5, 0, 0, 0, 0]);
    w.rec(0x012D, &[0]).rec(0x012D, &[1]);
    // Rectangle left half: bottom, right, top, left.
    w.rec(0x041B, &[1000, 500, 0, 0]);
    // Blue brush in slot 2, ellipse in the right half.
    let [c0, c1] = color(0, 0, 255);
    w.rec(0x02FC, &[0, c0, c1, 0]);
    w.rec(0x012D, &[2]);
    w.rec(0x0418, &[1000, 1000, 0, 500]);
    let wmf = w.finish(Some((0, 0, 1000, 1000, 1000)));
    assert_eq!(kind(&wmf), Some(Kind::Wmf));
    let pic = render(&wmf).expect("plays");
    let (pw, ph) = pic.size();
    assert!(
        (pw - 96.0).abs() < 0.5 && (ph - 96.0).abs() < 0.5,
        "{pw}×{ph}"
    );
    let png = pic.png(512).unwrap();
    assert!(near(pixel(&png, 0.25, 0.5), [255, 0, 0, 255]));
    assert!(near(pixel(&png, 0.75, 0.5), [0, 0, 255, 255]));
    // Ellipse corners stay empty.
    assert_eq!(pixel(&png, 0.52, 0.02)[3], 0);
}

#[test]
fn wmf_flipped_window_and_text() {
    let mut w = Wmf::new();
    // y goes up: origin at the bottom left (window org 0,1000; ext 1000,-1000).
    w.rec(0x0103, &[8]);
    w.rec(0x020B, &[1000, 0]).rec(0x020C, &[-1000, 1000]);
    let [c0, c1] = color(0, 128, 0);
    w.rec(0x02FC, &[0, c0, c1, 0]).rec(0x012D, &[0]);
    w.rec(0x02FA, &[5, 0, 0, 0, 0]).rec(0x012D, &[1]);
    // A rectangle near y = 1000 (the top, since y is up).
    w.rec(0x041B, &[1000, 1000, 800, 0]);
    // Text "Hi" with a font, transparent.
    let mut font = vec![];
    for v in [-120i16, 0, 0, 0, 400] {
        font.extend_from_slice(&v.to_le_bytes());
    }
    font.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 0]);
    let mut face = b"Arial".to_vec();
    face.resize(32, 0);
    font.extend_from_slice(&face);
    w.raw(0x02FB, &font).rec(0x012D, &[2]);
    w.rec(0x0102, &[1]);
    w.rec(0x0521, &[2, i16::from_le_bytes(*b"Hi"), 500, 100]);
    let wmf = w.finish(None);
    let pic = render(&wmf).expect("plays");
    assert!(pic.svg().contains(">Hi</text>"), "{}", pic.svg());
    let png = pic.png(400).unwrap();
    assert!(near(pixel(&png, 0.5, 0.05), [0, 128, 0, 255]));
    assert_eq!(pixel(&png, 0.5, 0.95)[3], 0);
}

// ---- EMF ------------------------------------------------------------------

struct Emf(Vec<u8>, u32);

impl Emf {
    fn new() -> Self {
        Emf(Vec::new(), 0)
    }
    fn rec(&mut self, ty: u32, data: &[u8]) -> &mut Self {
        let mut d = data.to_vec();
        while !d.len().is_multiple_of(4) {
            d.push(0);
        }
        self.0.extend_from_slice(&ty.to_le_bytes());
        self.0
            .extend_from_slice(&(8 + d.len() as u32).to_le_bytes());
        self.0.extend_from_slice(&d);
        self.1 += 1;
        self
    }
    fn ints(&mut self, ty: u32, v: &[i32]) -> &mut Self {
        let d: Vec<u8> = v.iter().flat_map(|x| x.to_le_bytes()).collect();
        self.rec(ty, &d)
    }
    /// A 100×100 px picture at 96 dpi (device 96 px per inch).
    fn finish(&mut self) -> Vec<u8> {
        self.ints(14, &[0, 0, 20]);
        let mut h = Vec::new();
        for v in [0i32, 0, 99, 99] {
            h.extend_from_slice(&v.to_le_bytes());
        }
        // Frame in .01 mm: 100 px at 96 dpi = 26.458 mm.
        for v in [0i32, 0, 2646, 2646] {
            h.extend_from_slice(&v.to_le_bytes());
        }
        h.extend_from_slice(&0x464D_4520u32.to_le_bytes());
        h.extend_from_slice(&0x10000u32.to_le_bytes());
        h.extend_from_slice(&0u32.to_le_bytes()); // bytes (unchecked)
        h.extend_from_slice(&(self.1 + 1).to_le_bytes());
        h.extend_from_slice(&8u16.to_le_bytes());
        h.extend_from_slice(&0u16.to_le_bytes());
        h.extend_from_slice(&[0; 12]); // description, palette
        for v in [1920i32, 1080, 508, 286] {
            h.extend_from_slice(&v.to_le_bytes());
        }
        let mut out = Vec::new();
        out.extend_from_slice(&1u32.to_le_bytes());
        out.extend_from_slice(&(8 + h.len() as u32).to_le_bytes());
        out.extend_from_slice(&h);
        out.extend_from_slice(&self.0);
        out
    }
}

fn cref(r: u8, g: u8, b: u8) -> i32 {
    i32::from_le_bytes([r, g, b, 0])
}

#[test]
fn emf_shapes_bitmap_and_clip() {
    let mut e = Emf::new();
    // Brush 1: green; pen: stock null pen.
    e.ints(39, &[1, 0, cref(0, 200, 0), 0]);
    e.ints(37, &[1]).ints(37, &[0x8000_0008u32 as i32]);
    // Top-left quarter.
    e.ints(43, &[0, 0, 50, 50]);
    // World transform: move by (50, 0), then another rectangle in red.
    e.ints(39, &[2, 0, cref(255, 0, 0), 0]).ints(37, &[2]);
    let xf: Vec<u8> = [1.0f32, 0.0, 0.0, 1.0, 50.0, 0.0]
        .iter()
        .flat_map(|v| v.to_le_bytes())
        .collect();
    e.rec(35, &xf);
    e.ints(43, &[0, 0, 50, 50]);
    e.rec(
        35,
        &[1.0f32, 0.0, 0.0, 1.0, 0.0, 0.0]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect::<Vec<u8>>(),
    );
    // A 2×1 bitmap (blue, yellow) over the bottom half via StretchDIBits.
    let mut bmi = Vec::new();
    for v in [40u32, 2, 1] {
        bmi.extend_from_slice(&v.to_le_bytes());
    }
    bmi.extend_from_slice(&1u16.to_le_bytes());
    bmi.extend_from_slice(&24u16.to_le_bytes());
    bmi.extend_from_slice(&[0; 24]);
    let bits = [255u8, 0, 0, 0, 255, 255, 0, 0]; // BGR blue, BGR yellow, pad
    let mut d = Vec::new();
    for v in [0i32, 50, 99, 99] {
        d.extend_from_slice(&v.to_le_bytes()); // bounds
    }
    let off_bmi = 8 + 72;
    let off_bits = off_bmi + bmi.len() as u32;
    for v in [0i32, 50, 0, 0, 2, 1] {
        d.extend_from_slice(&v.to_le_bytes());
    }
    for v in [
        off_bmi,
        bmi.len() as u32,
        off_bits,
        bits.len() as u32,
        0,
        0x00CC_0020,
    ] {
        d.extend_from_slice(&v.to_le_bytes());
    }
    for v in [100i32, 50] {
        d.extend_from_slice(&v.to_le_bytes());
    }
    d.extend_from_slice(&bmi);
    d.extend_from_slice(&bits);
    e.rec(81, &d);
    let emf = e.finish();
    assert_eq!(kind(&emf), Some(Kind::Emf));
    let (w, h) = natural_size(&emf).unwrap();
    assert!(
        (w - 100.0).abs() < 1.0 && (h - 100.0).abs() < 1.0,
        "{w}×{h}"
    );
    let png = to_png(&emf, 400).unwrap();
    assert!(near(pixel(&png, 0.25, 0.25), [0, 200, 0, 255]));
    assert!(near(pixel(&png, 0.75, 0.25), [255, 0, 0, 255]));
    assert!(near(pixel(&png, 0.25, 0.75), [0, 0, 255, 255]));
    assert!(near(pixel(&png, 0.75, 0.75), [255, 255, 0, 255]));
}

#[test]
fn emf_clip_text_and_map_mode() {
    let mut e = Emf::new();
    // Anisotropic: logical 0..1000 → device 0..100.
    e.ints(17, &[8]);
    e.ints(9, &[1000, 1000]).ints(11, &[100, 100]);
    e.ints(39, &[1, 0, cref(0, 0, 255), 0]).ints(37, &[1]);
    e.ints(37, &[0x8000_0008u32 as i32]);
    // Clip to the left half, then fill everything.
    e.ints(30, &[0, 0, 500, 1000]);
    e.ints(43, &[0, 0, 1000, 1000]);
    // Font and text (transparent background).
    let mut f = Vec::new();
    f.extend_from_slice(&2u32.to_le_bytes());
    for v in [-100i32, 0, 0, 0, 700] {
        f.extend_from_slice(&v.to_le_bytes());
    }
    f.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 0]);
    let face: Vec<u8> = "Arial"
        .encode_utf16()
        .chain(std::iter::repeat(0))
        .take(32)
        .flat_map(|u| u.to_le_bytes())
        .collect();
    f.extend_from_slice(&face);
    e.rec(82, &f).ints(37, &[2]);
    e.ints(18, &[1]);
    let text: Vec<u8> = "Kantor"
        .encode_utf16()
        .flat_map(|u| u.to_le_bytes())
        .collect();
    let mut t = Vec::new();
    for v in [0i32, 0, 0, 0, 1] {
        t.extend_from_slice(&v.to_le_bytes()); // bounds, graphics mode
    }
    t.extend_from_slice(&1f32.to_le_bytes());
    t.extend_from_slice(&1f32.to_le_bytes());
    for v in [100i32, 600, 6, 76, 0, 0, 0, 0, 0, 0] {
        t.extend_from_slice(&v.to_le_bytes());
    }
    t.extend_from_slice(&text);
    e.rec(84, &t);
    let emf = e.finish();
    let pic = render(&emf).unwrap();
    assert!(pic.svg().contains(">Kantor</text>"), "{}", pic.svg());
    assert!(pic.svg().contains("font-weight=\"bold\""));
    let png = pic.png(400).unwrap();
    assert!(near(pixel(&png, 0.25, 0.1), [0, 0, 255, 255]));
    assert_eq!(pixel(&png, 0.75, 0.1)[3], 0, "clipped away");
}

fn plus_record(ty: u16, flags: u16, data: &[u8]) -> Vec<u8> {
    let mut r = Vec::new();
    r.extend_from_slice(&ty.to_le_bytes());
    r.extend_from_slice(&flags.to_le_bytes());
    r.extend_from_slice(&(12 + data.len() as u32).to_le_bytes());
    r.extend_from_slice(&(data.len() as u32).to_le_bytes());
    r.extend_from_slice(data);
    r
}

fn plus_comment(e: &mut Emf, records: &[u8]) {
    let mut d = Vec::new();
    d.extend_from_slice(&(4 + records.len() as u32).to_le_bytes());
    d.extend_from_slice(&0x2B46_4D45u32.to_le_bytes());
    d.extend_from_slice(records);
    e.rec(70, &d);
}

fn plus_header(dual: bool) -> Vec<u8> {
    let mut d = Vec::new();
    for v in [0xDBC0_1002u32, 1, 96, 96] {
        d.extend_from_slice(&v.to_le_bytes());
    }
    plus_record(0x4001, dual as u16, &d)
}

#[test]
fn emf_plus_only_draws_plus_records() {
    let mut e = Emf::new();
    let mut recs = plus_header(false);
    // Page unit pixel; FillRects with an ARGB colour (S flag): magenta.
    recs.extend(plus_record(0x4030, 2, &1f32.to_le_bytes()));
    let mut d = Vec::new();
    d.extend_from_slice(&0xFFFF_00FFu32.to_le_bytes());
    d.extend_from_slice(&1u32.to_le_bytes());
    for v in [0f32, 0.0, 100.0, 50.0] {
        d.extend_from_slice(&v.to_le_bytes());
    }
    recs.extend(plus_record(0x400A, 0x8000, &d));
    plus_comment(&mut e, &recs);
    // A GDI record that an EMF+ only reader skips.
    e.ints(39, &[1, 0, cref(0, 0, 0), 0]).ints(37, &[1]);
    e.ints(43, &[0, 50, 100, 100]);
    let png = to_png(&e.finish(), 200).unwrap();
    assert!(near(pixel(&png, 0.5, 0.25), [255, 0, 255, 255]));
    assert_eq!(pixel(&png, 0.5, 0.75)[3], 0);
}

#[test]
fn emf_plus_dual_draws_emf_records() {
    let mut e = Emf::new();
    let mut recs = plus_header(true);
    let mut d = Vec::new();
    d.extend_from_slice(&0xFFFF_00FFu32.to_le_bytes());
    d.extend_from_slice(&1u32.to_le_bytes());
    for v in [0f32, 0.0, 100.0, 50.0] {
        d.extend_from_slice(&v.to_le_bytes());
    }
    recs.extend(plus_record(0x400A, 0x8000, &d));
    plus_comment(&mut e, &recs);
    e.ints(39, &[1, 0, cref(0, 0, 0), 0]).ints(37, &[1]);
    e.ints(37, &[0x8000_0008u32 as i32]);
    e.ints(43, &[0, 50, 100, 100]);
    let png = to_png(&e.finish(), 200).unwrap();
    assert_eq!(pixel(&png, 0.5, 0.25)[3], 0);
    assert!(near(pixel(&png, 0.5, 0.75), [0, 0, 0, 255]));
}

// ---- PICT and BMP -------------------------------------------------------------

#[test]
fn pict_v2_paint_rect() {
    let mut p = vec![0u8; 512];
    p.extend_from_slice(&0u16.to_be_bytes());
    for v in [0i16, 0, 72, 144] {
        p.extend_from_slice(&v.to_be_bytes()); // top left bottom right
    }
    p.extend_from_slice(&[0x00, 0x11, 0x02, 0xFF]);
    p.extend_from_slice(&0x0C00u16.to_be_bytes());
    p.extend_from_slice(&[0; 24]);
    // RGBFgCol orange, PaintRect over the left half.
    p.extend_from_slice(&0x001Au16.to_be_bytes());
    for v in [0xFFFFu16, 0x8000, 0] {
        p.extend_from_slice(&v.to_be_bytes());
    }
    p.extend_from_slice(&0x0031u16.to_be_bytes());
    for v in [0i16, 0, 72, 72] {
        p.extend_from_slice(&v.to_be_bytes());
    }
    p.extend_from_slice(&0x00FFu16.to_be_bytes());
    assert_eq!(kind(&p), Some(Kind::Pict));
    let (w, h) = natural_size(&p).unwrap();
    assert_eq!((w.round(), h.round()), (192.0, 96.0));
    let png = to_png(&p, 400).unwrap();
    assert!(near(pixel(&png, 0.25, 0.5), [255, 128, 0, 255]));
    assert_eq!(pixel(&png, 0.75, 0.5)[3], 0);
}

#[test]
fn bmp_and_dib() {
    // 2×2, 24-bit, bottom-up: bottom row red, green; top row blue, white.
    let mut dib = Vec::new();
    for v in [40u32, 2, 2] {
        dib.extend_from_slice(&v.to_le_bytes());
    }
    dib.extend_from_slice(&1u16.to_le_bytes());
    dib.extend_from_slice(&24u16.to_le_bytes());
    dib.extend_from_slice(&[0; 24]);
    dib.extend_from_slice(&[0, 0, 255, 0, 255, 0, 0, 0]);
    dib.extend_from_slice(&[255, 0, 0, 255, 255, 255, 0, 0]);
    let png = kantor_metafile::dib_to_png(&dib).unwrap();
    assert!(near(pixel(&png, 0.25, 0.75), [255, 0, 0, 255]));
    assert!(near(pixel(&png, 0.25, 0.25), [0, 0, 255, 255]));
    let bmp = kantor_metafile::dib_to_bmp(&dib);
    assert_eq!(kind(&bmp), Some(Kind::Bmp));
    let png2 = to_png(&bmp, 100).unwrap();
    assert!(near(pixel(&png2, 0.75, 0.25), [255, 255, 255, 255]));
}

#[test]
fn garbage_is_refused_not_panicking() {
    assert!(kind(b"hello").is_none());
    assert!(to_png(b"", 100).is_none());
    // A truncated EMF header.
    let mut e = Emf::new().finish();
    e.truncate(60);
    let _ = to_png(&e, 100);
    // Random bytes after a WMF header.
    let mut w = Wmf::new().finish(Some((0, 0, 100, 100, 1440)));
    w.truncate(w.len() - 6);
    w.extend((0..400u32).map(|i| (i * 7919 % 251) as u8));
    let _ = to_png(&w, 100);
}
