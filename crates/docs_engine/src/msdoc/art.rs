//! Floating objects of a `.doc` ([MS-DOC] 2.9.253 PlcfSpa, [MS-ODRAW]).
//!
//! Each floating object has an anchor character (`0x08`) in the text and
//! an `SPA` in `PlcSpaMom` at that CP: the shape id, its rectangle in twips
//! from what `bx`/`by` say (margin, page, column / paragraph), and its
//! wrapping. The shape itself is in the OfficeArt drawing (`DggInfo` in the
//! table stream): its type, properties (fill, line, picture, crop, text
//! insets, name) and, for a text box, the id of its text in the text box
//! story. Pictures are in the BLIP store, either in the `FBSE` itself or in
//! the WordDocument stream at `foDelay`.
//!
//! Preset shape types become DrawingML presets; freeform shapes (which
//! LibreOffice writes for all its shapes) become a `custGeom` path from
//! their vertices and segments, vertices that name guide formulas worked
//! out ([`guides`]).

use std::collections::HashMap;

use super::rd::{i32_at, u16_at, u32_at};

pub(crate) use kantor_ooxml::officeart::{arrowhead, color, preset};

/// An `SPA`: where a floating object is anchored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Spa {
    pub spid: u32,
    /// Left, top, right, bottom in twips.
    pub rect: [i32; 4],
    /// 0 margin, 1 page, 2 column (`bx`); 0 margin, 1 page, 2 paragraph (`by`).
    pub bx: u8,
    pub by: u8,
    /// 0 around, 1 top and bottom, 2 square, 3 none, 4 tight, 5 through.
    pub wr: u8,
    /// 0 both sides, 1 left, 2 right, 3 largest.
    pub wrk: u8,
    pub below: bool,
}

/// `PlcSpaMom` (or `PlcSpaHdr`): SPAs by CP.
pub(crate) fn spas(plc: &[u8]) -> HashMap<u32, Spa> {
    let mut out = HashMap::new();
    if plc.len() < 4 {
        return out;
    }
    let n = (plc.len() - 4) / 30;
    for k in 0..n {
        let at = (n + 1) * 4 + k * 26;
        let (Some(cp), Some(spid), Some(flags)) =
            (u32_at(plc, k * 4), u32_at(plc, at), u16_at(plc, at + 20))
        else {
            break;
        };
        let r = |o: usize| i32_at(plc, at + o).unwrap_or(0);
        out.insert(
            cp,
            Spa {
                spid,
                rect: [r(4), r(8), r(12), r(16)],
                bx: ((flags >> 1) & 3) as u8,
                by: ((flags >> 3) & 3) as u8,
                wr: ((flags >> 5) & 15) as u8,
                wrk: ((flags >> 9) & 15) as u8,
                below: flags & 0x4000 != 0,
            },
        );
    }
    out
}

fn read_props(r: &Rec, s: &mut Shape) {
    kantor_ooxml::officeart::read_fopt(r.inst as usize, r.body, &mut s.props, &mut s.complex);
}

/// A freeform shape's path as `a:custGeom` (see
/// [`kantor_ooxml::officeart::cust_geom`]).
pub(crate) fn cust_geom(s: &Shape) -> Option<String> {
    kantor_ooxml::officeart::cust_geom(&s.props, &s.complex)
}

/// A shape of the drawing.
#[derive(Clone, Debug, Default)]
pub(crate) struct Shape {
    /// `msospt` (the FSP record's instance).
    pub typ: u16,
    pub flags: u32,
    /// Simple properties by id.
    pub props: HashMap<u16, u32>,
    /// Complex properties' data by id.
    pub complex: HashMap<u16, Vec<u8>>,
    /// `OfficeArtClientTextbox`: the text box's id.
    pub txid: Option<u32>,
    /// Drawing order in its group.
    pub order: usize,
    /// A group's shapes (ids, back to front).
    pub children: Vec<u32>,
    /// A group's own coordinates (`OfficeArtFSPGR`): left, top, right,
    /// bottom.
    pub group_rect: Option<[i32; 4]>,
    /// A shape's place in its group's coordinates (`OfficeArtChildAnchor`).
    pub child_anchor: Option<[i32; 4]>,
}

impl Shape {
    pub fn group(&self) -> bool {
        self.flags & 0x1 != 0
    }
    pub fn flip(&self) -> (bool, bool) {
        (self.flags & 0x40 != 0, self.flags & 0x80 != 0)
    }
    pub fn prop(&self, id: u16) -> Option<u32> {
        self.props.get(&id).copied()
    }
    /// A boolean of a property set (`fUse…` bit 16 above the value bit).
    pub fn flag(&self, id: u16, bit: u32) -> Option<bool> {
        let v = self.prop(id)?;
        (v & (1 << (bit + 16)) != 0).then_some(v & (1 << bit) != 0)
    }
    /// A UTF-16 string property (name, description).
    pub fn text(&self, id: u16) -> Option<String> {
        let b = self.complex.get(&id)?;
        let units: Vec<u16> = b
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u16::from_le_bytes(*c))
            .take_while(|&u| u != 0)
            .collect();
        let s = String::from_utf16_lossy(&units);
        (!s.trim().is_empty()).then_some(s)
    }
}

/// A picture of the BLIP store.
#[derive(Clone, Debug)]
pub(crate) enum Blip {
    Image(&'static str, Vec<u8>),
    /// Metafiles, DIB, TIFF.
    Other,
}

/// One OfficeArt record: version, instance, type, body.
struct Rec<'a> {
    ver: u16,
    inst: u16,
    typ: u16,
    body: &'a [u8],
}

fn records(mut b: &[u8]) -> Vec<Rec<'_>> {
    let mut out = Vec::new();
    while b.len() >= 8 {
        let (Some(vi), Some(typ), Some(len)) = (u16_at(b, 0), u16_at(b, 2), u32_at(b, 4)) else {
            break;
        };
        let end = 8usize.saturating_add(len as usize).min(b.len());
        out.push(Rec {
            ver: vi & 0xF,
            inst: vi >> 4,
            typ,
            body: &b[8..end],
        });
        b = &b[end..];
    }
    out
}

/// The drawing of the main document (`dgglbl` 0) and the BLIP store.
#[derive(Default)]
pub(crate) struct Drawing {
    pub shapes: HashMap<u32, Shape>,
    pub blips: Vec<Blip>,
}

/// Reads `DggInfo`; `word` is the WordDocument stream (`foDelay` blips).
pub(crate) fn drawing(dgg: &[u8], word: &[u8]) -> Drawing {
    let mut d = Drawing::default();
    let mut b = dgg;
    // OfficeArtDggContainer, then (dgglbl, OfficeArtDgContainer) pairs.
    let mut first = true;
    while b.len() >= 8 {
        if !first {
            let label = b[0];
            b = &b[1..];
            let Some(len) = u32_at(b, 4) else { break };
            let end = 8usize.saturating_add(len as usize).min(b.len());
            // dgglbl 0: the main text's drawing, 1: the headers' (shape
            // ids are unique across both).
            if label <= 1 && u16_at(b, 2) == Some(0xF002) {
                let mut order = 0;
                let _ = shapes(&b[8..end], &mut d.shapes, &mut order);
            }
            b = &b[end..];
            continue;
        }
        first = false;
        let Some(len) = u32_at(b, 4) else { break };
        let end = 8usize.saturating_add(len as usize).min(b.len());
        if u16_at(b, 2) == Some(0xF000) {
            for store in records(&b[8..end]).into_iter().filter(|r| r.typ == 0xF001) {
                for fbse in records(store.body) {
                    d.blips.push(if fbse.typ == 0xF007 {
                        fbse_blip(fbse.body, word)
                    } else {
                        Blip::Other
                    });
                }
            }
        }
        b = &b[end..];
    }
    d
}

fn fbse_blip(body: &[u8], word: &[u8]) -> Blip {
    let name = body.get(33).copied().unwrap_or(0) as usize;
    let rec = if body.len() > 36 + name + 8 {
        body.get(36 + name..)
    } else {
        let size = u32_at(body, 20).unwrap_or(0) as usize;
        u32_at(body, 28).and_then(|at| {
            let at = at as usize;
            word.get(at..at.checked_add(size)?.min(word.len()))
        })
    };
    match rec.and_then(super::blip_record) {
        Some((ext, bytes)) => Blip::Image(ext, bytes),
        None => Blip::Other,
    }
}

/// The shapes of a drawing container, by id (groups and their children);
/// returns the ids of the shapes and groups directly in `b`.
fn shapes(b: &[u8], out: &mut HashMap<u32, Shape>, order: &mut usize) -> Vec<u32> {
    let rect = |body: &[u8]| -> Option<[i32; 4]> {
        Some([
            i32_at(body, 0)?,
            i32_at(body, 4)?,
            i32_at(body, 8)?,
            i32_at(body, 12)?,
        ])
    };
    let mut ids = Vec::new();
    for r in records(b) {
        match r.typ {
            // Group container: its first shape is the group itself.
            0xF003 => {
                let inner = shapes(r.body, out, order);
                if let Some((&group, rest)) = inner.split_first() {
                    if let Some(g) = out.get_mut(&group) {
                        g.children = rest.to_vec();
                    }
                    ids.push(group);
                }
            }
            0xF004 => {
                let mut s = Shape {
                    order: *order,
                    ..Shape::default()
                };
                *order += 1;
                let mut spid = None;
                for c in records(r.body) {
                    match c.typ {
                        0xF00A => {
                            s.typ = c.inst;
                            spid = u32_at(c.body, 0);
                            s.flags = u32_at(c.body, 4).unwrap_or(0);
                        }
                        0xF009 => s.group_rect = rect(c.body),
                        0xF00F => s.child_anchor = rect(c.body),
                        0xF00B | 0xF121 | 0xF122 => read_props(&c, &mut s),
                        0xF00D => s.txid = u32_at(c.body, 0),
                        _ => {}
                    }
                }
                // Text linked by property (`lTxid`) rather than by record.
                if s.txid.is_none() {
                    s.txid = s.prop(0x80);
                }
                if let Some(id) = spid {
                    out.insert(id, s);
                    ids.push(id);
                }
            }
            _ if r.ver == 0xF => ids.extend(shapes(r.body, out, order)),
            _ => {}
        }
    }
    ids
}

/// `PlcftxbxTxt`: each text box's CP range in the text box story (the
/// last entry is a guard) and the shape id it belongs to.
pub(crate) fn text_boxes(plc: &[u8]) -> Vec<(u32, u32, u32)> {
    let mut out = Vec::new();
    if plc.len() < 4 {
        return out;
    }
    let n = (plc.len() - 4) / 26;
    for k in 0..n {
        let (Some(a), Some(b)) = (u32_at(plc, k * 4), u32_at(plc, k * 4 + 4)) else {
            break;
        };
        let lid = u32_at(plc, (n + 1) * 4 + k * 22 + 14).unwrap_or(0);
        if b > a {
            out.push((a, b, lid));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colours_and_flags() {
        assert_eq!(color(0x00FF_EBDD), Some(0xDDEBFF));
        assert_eq!(color(0x0800_0001), None);
        let mut s = Shape::default();
        s.props.insert(0x1FF, 0x80008);
        assert_eq!(s.flag(0x1FF, 3), Some(true));
        s.props.insert(0x1BF, 0x100000);
        assert_eq!(s.flag(0x1BF, 4), Some(false));
    }

    #[test]
    fn freeform_path() {
        let mut s = Shape::default();
        // Three i16 points: (0,0), (100,0), (100,50); segments move, 2 lines, close, end.
        let mut v = vec![3, 0, 3, 0, 0xF0, 0xFF];
        for (x, y) in [(0i16, 0i16), (100, 0), (100, 50)] {
            v.extend(x.to_le_bytes());
            v.extend(y.to_le_bytes());
        }
        s.complex.insert(0x145, v);
        let mut g = vec![4, 0, 4, 0, 2, 0];
        for seg in [0x4000u16, 0x0002, 0x6001, 0x8000] {
            g.extend(seg.to_le_bytes());
        }
        s.complex.insert(0x146, g);
        s.props.insert(0x142, 0);
        s.props.insert(0x143, 0);
        let x = cust_geom(&s).unwrap();
        assert!(x.contains("<a:path w=\"100\" h=\"50\">"), "{x}");
        assert!(
            x.contains("<a:moveTo><a:pt x=\"0\" y=\"0\"/></a:moveTo><a:lnTo>"),
            "{x}"
        );
        assert!(x.contains("<a:close/>"), "{x}");
    }
}
