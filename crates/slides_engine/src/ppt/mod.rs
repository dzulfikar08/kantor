//! PowerPoint 97-2003 (`.ppt`, `.pps`, `.pot`) import, read-only.
//!
//! The binary file ([MS-PPT], with OfficeArt drawings from [MS-ODRAW]) is
//! read and written out as an equivalent `.pptx` package, which
//! [`Presentation::from_bytes`](crate::Presentation::from_bytes) then opens
//! like any other. Going through a package (rather than building the model
//! directly) keeps one loader: masters, layouts, themes, notes and pictures
//! get the same inheritance, rendering and round-trip rules as a `.pptx`,
//! and Save writes that package as a new `.pptx` file.
//!
//! What is read: the slide list and size, masters (background, shapes,
//! placeholders, the title/body/other text styles, colour scheme) and
//! title masters (as a "Title Slide" layout), slides (shapes with position,
//! size, rotation, flips, preset or freeform geometry, solid, gradient and
//! picture fills, outlines with dashes and arrowheads, text with paragraph
//! and character formatting, placeholders, pictures, groups, tables),
//! hidden slides and speaker notes. Transitions, animations, sounds,
//! embedded fonts and macros are left out; OLE objects show their preview
//! picture. EMF/WMF pictures are kept in the package (PowerPoint shows them
//! after Save) and appear in Kantor as a grey box.
//!
//! Every offset and length comes from the file, so all reads are checked
//! ([`rec`]); a malformed file gives an error, never a panic.

mod draw;
mod rec;
mod table;
mod text;

use std::collections::HashMap;
use std::io::{Cursor, Read as _, Write as _};

use anyhow::{Context as _, anyhow, bail};

use draw::{Part, Props, Shape};
use rec::{Cur, Rec, rt};
use text::{Styles, Text, tx};

/// EMUs per master unit (576 per inch).
const MU_EMU: f64 = 914_400.0 / 576.0;

/// File extensions of the binary format.
pub const EXTENSIONS: [&str; 3] = ["ppt", "pps", "pot"];

/// Whether `path` names a PowerPoint 97-2003 file (by extension).
pub fn is_legacy_path(path: &std::path::Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| EXTENSIONS.iter().any(|x| x.eq_ignore_ascii_case(e)))
}

/// Whether `bytes` start like a compound file (the container of .ppt).
pub fn is_compound_file(bytes: &[u8]) -> bool {
    bytes.starts_with(&[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1])
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Color {
    Rgb(u8, u8, u8),
    Scheme(u8),
}

/// What text and colours need: fonts and the colour scheme in use.
#[derive(Clone, Copy)]
struct Ctx<'a> {
    pub fonts: &'a [String],
    pub scheme: [[u8; 3]; 8],
}

impl Ctx<'_> {
    pub fn rgb(&self, c: Color) -> Option<String> {
        let [r, g, b] = match c {
            Color::Rgb(r, g, b) => [r, g, b],
            Color::Scheme(i) => *self.scheme.get(i as usize)?,
        };
        Some(format!("{r:02X}{g:02X}{b:02X}"))
    }

    pub fn font(&self, i: u16) -> Option<String> {
        self.fonts
            .get(i as usize)
            .filter(|f| !f.is_empty())
            .cloned()
    }
}

fn esc(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '"' => o.push_str("&quot;"),
            '\'' => o.push_str("&apos;"),
            c if (c as u32) < 0x20 && c != '\t' && c != '\n' && c != '\r' => {}
            '\u{FFFE}' | '\u{FFFF}' => {}
            c => o.push(c),
        }
    }
    o
}

/// A picture from the BStore.
struct Blip {
    ext: &'static str,
    data: Vec<u8>,
    /// Kantor can draw it (not EMF/WMF/PICT).
    renderable: bool,
}

/// Document-wide things shapes need.
struct Doc<'a> {
    pub fonts: Vec<String>,
    /// The default text styles (DocumentContainer → Environment).
    pub styles: Styles,
    /// Default shape properties of the drawing group.
    pub dgg_defaults: Props<'a>,
    /// Master shapes' properties by shape id (`hspMaster`).
    pub master_props: HashMap<u32, Props<'a>>,
    pub blips: Vec<Option<Blip>>,
}

/// Pictures written to `ppt/media`.
#[derive(Default)]
struct Media {
    parts: Vec<(String, Vec<u8>)>,
    by_pib: HashMap<u32, (String, bool)>,
    /// Pictures used that Kantor can't draw.
    pub hidden: usize,
}

impl Media {
    /// The media file name of picture `pib` (1-based), written once.
    pub fn part_for(&mut self, doc: &Doc, pib: u32) -> Option<String> {
        if let Some((n, shown)) = self.by_pib.get(&pib) {
            if !shown {
                self.hidden += 1;
            }
            return Some(n.clone());
        }
        let blip = doc.blips.get((pib as usize).checked_sub(1)?)?.as_ref()?;
        let name = format!("image{}.{}", self.parts.len() + 1, blip.ext);
        self.parts.push((name.clone(), blip.data.clone()));
        self.by_pib.insert(pib, (name.clone(), blip.renderable));
        if !blip.renderable {
            self.hidden += 1;
        }
        Some(name)
    }

    pub fn renderable(&self, pib: u32) -> bool {
        self.by_pib.get(&pib).is_none_or(|(_, r)| *r)
    }
}

/// The result of an import: a `.pptx` package and notes about what could
/// not be brought over.
pub struct Imported {
    pub pptx: Vec<u8>,
    pub notes: Vec<String>,
}

fn damaged() -> anyhow::Error {
    anyhow!("it is damaged or not a PowerPoint 97-2003 presentation")
}

fn old_version() -> anyhow::Error {
    anyhow!(
        "it was saved by PowerPoint 95 or older, which Kantor can't read; open it in a newer PowerPoint and save it as .pptx"
    )
}

fn encrypted() -> anyhow::Error {
    anyhow!(
        "it is password-protected; Kantor can't open encrypted PowerPoint files. Remove the password in PowerPoint, or save it there as .pptx"
    )
}

fn read_stream<F: std::io::Read + std::io::Seek>(
    cf: &mut cfb::CompoundFile<F>,
    name: &str,
) -> anyhow::Result<Option<Vec<u8>>> {
    if !cf.is_stream(name) {
        return Ok(None);
    }
    let mut s = cf.open_stream(name).map_err(|_| damaged())?;
    let mut v = Vec::new();
    s.read_to_end(&mut v).map_err(|_| damaged())?;
    Ok(Some(v))
}

/// Convert a `.ppt` file's bytes into a `.pptx` package.
pub fn convert(bytes: &[u8]) -> anyhow::Result<Imported> {
    if !is_compound_file(bytes) {
        bail!("it is not a PowerPoint 97-2003 presentation");
    }
    let mut cf = cfb::CompoundFile::open(Cursor::new(bytes)).map_err(|_| damaged())?;
    if cf.is_stream("EncryptedSummary") {
        return Err(encrypted());
    }
    let Some(doc_stream) = read_stream(&mut cf, "PowerPoint Document")? else {
        if cf.exists("PP40") || cf.exists("Header") {
            return Err(old_version());
        }
        bail!("it is not a PowerPoint presentation (no slides inside)");
    };
    let Some(cur_user) = read_stream(&mut cf, "Current User")? else {
        return Err(old_version());
    };
    let pictures = read_stream(&mut cf, "Pictures")?.unwrap_or_default();
    convert_streams(&doc_stream, &cur_user, &pictures)
}

/// Slide-to-container offsets from the persist directories of every edit.
fn persist_map(doc: &[u8], mut edit: u32) -> anyhow::Result<(HashMap<u32, u32>, u32)> {
    let mut map: HashMap<u32, u32> = HashMap::new();
    let mut seen = std::collections::HashSet::new();
    let mut doc_ref = None;
    while seen.insert(edit) && seen.len() < 10_000 {
        let r = Rec::at(doc, edit as usize).ok_or_else(damaged)?;
        if r.ty != rt::USER_EDIT {
            return Err(damaged());
        }
        if r.data.len() >= 0x20 {
            return Err(encrypted());
        }
        let mut c = r.cur();
        let (_last_slide, _ver, _minor, major) = (c.u32(), c.u16(), c.u8(), c.u8());
        if major.is_some_and(|m| m < 3) {
            return Err(old_version());
        }
        let last = c.u32().ok_or_else(damaged)?;
        let dir = c.u32().ok_or_else(damaged)?;
        let docr = c.u32().ok_or_else(damaged)?;
        doc_ref.get_or_insert(docr);
        if let Some(d) = Rec::at(doc, dir as usize).filter(|d| d.ty == rt::PERSIST_DIRECTORY) {
            let mut c = d.cur();
            while let Some(v) = c.u32() {
                let (start, n) = (v & 0xF_FFFF, v >> 20);
                for k in 0..n {
                    let Some(off) = c.u32() else { break };
                    map.entry(start + k).or_insert(off);
                }
            }
        }
        if last == 0 {
            break;
        }
        edit = last;
    }
    Ok((map, doc_ref.ok_or_else(damaged)?))
}

fn scheme_of(r: &Rec) -> Option<[[u8; 3]; 8]> {
    let s = r
        .kids(rt::COLOR_SCHEME)
        .find(|s| s.inst == 1)
        .or_else(|| r.kids(rt::COLOR_SCHEME).next())?;
    let mut c = s.cur();
    let mut out = [[0u8; 3]; 8];
    for o in &mut out {
        let b = c.bytes(4)?;
        *o = [b[0], b[1], b[2]];
    }
    Some(out)
}

const DEFAULT_SCHEME: [[u8; 3]; 8] = [
    [0xFF, 0xFF, 0xFF],
    [0x00, 0x00, 0x00],
    [0x80, 0x80, 0x80],
    [0x00, 0x00, 0x00],
    [0xBB, 0xE0, 0xE3],
    [0x33, 0x33, 0x99],
    [0x00, 0x99, 0x99],
    [0x99, 0xCC, 0x00],
];

/// A master from the master list: a main master, or a title master.
struct MasterSrc<'a> {
    id: u32,
    rec: Rec<'a>,
    /// For a title master: the main master it belongs to.
    main: Option<u32>,
    flags: u16,
}

struct SlideSrc<'a> {
    id: u32,
    rec: Rec<'a>,
    texts: Vec<Text>,
    master: u32,
    geom: u32,
    flags: u16,
    hidden: bool,
}

/// `SlideAtom`: layout geometry, master id, flags.
fn slide_atom(r: &Rec) -> (u32, u32, u16) {
    let Some(a) = r.child(rt::SLIDE_ATOM) else {
        return (1, 0, 0x7);
    };
    let mut c = a.cur();
    let geom = c.u32().unwrap_or(1);
    let _ = c.skip(8);
    let master = c.u32().unwrap_or(0);
    let _notes = c.u32();
    let flags = c.u16().unwrap_or(0x7);
    (geom, master, flags)
}

/// The persisted slides of a SlideListWithText: `(persist id, slide id,
/// texts)`.
fn slide_list<'a>(r: &Rec<'a>) -> Vec<(u32, u32, Vec<Text>)> {
    let mut out: Vec<(u32, u32, Vec<Text>)> = Vec::new();
    let mut block: Vec<Rec<'a>> = Vec::new();
    let flush = |block: &mut Vec<Rec<'a>>, out: &mut Vec<(u32, u32, Vec<Text>)>| {
        if let Some(last) = out.last_mut()
            && !block.is_empty()
            && let Some(t) = Text::read(block.iter().copied(), tx::OTHER)
        {
            last.2.push(t);
        }
        block.clear();
    };
    for c in r.children() {
        match c.ty {
            rt::SLIDE_PERSIST_ATOM => {
                flush(&mut block, &mut out);
                let mut cur = c.cur();
                let pid = cur.u32().unwrap_or(0);
                let _ = cur.skip(8);
                let sid = cur.u32().unwrap_or(0);
                out.push((pid, sid, Vec::new()));
            }
            rt::TEXT_HEADER => {
                flush(&mut block, &mut out);
                block.push(c);
            }
            _ => block.push(c),
        }
    }
    flush(&mut block, &mut out);
    out
}

fn font_names(env: Option<&Rec>) -> Vec<String> {
    let Some(fc) = env.and_then(|e| e.child(rt::FONT_COLLECTION)) else {
        return Vec::new();
    };
    fc.kids(rt::FONT_ENTITY)
        .map(|f| {
            let v: Vec<u16> = f
                .data
                .get(..64)
                .unwrap_or(f.data)
                .as_chunks::<2>()
                .0
                .iter()
                .map(|c| u16::from_le_bytes(*c))
                .take_while(|&c| c != 0)
                .collect();
            String::from_utf16_lossy(&v)
        })
        .collect()
}

/// Read the BStore's pictures (from the Pictures stream or embedded).
fn read_blips(dgg: Option<&Rec>, pictures: &[u8]) -> Vec<Option<Blip>> {
    let Some(store) = dgg.and_then(|d| d.child(rt::BSTORE)) else {
        return Vec::new();
    };
    store
        .children()
        .map(|f| {
            if f.ty != rt::FBSE {
                return None;
            }
            let mut c = f.cur();
            let _ = c.skip(2 + 16 + 2);
            let size = c.u32()?;
            let _cref = c.u32()?;
            let delay = c.u32()?;
            let _ = c.skip(1);
            let name_len = c.u8()? as usize;
            let _ = c.skip(2 + name_len);
            let embedded = c.rest();
            let rec = if !embedded.is_empty() {
                Rec::at(embedded, 0)
            } else if size > 0 {
                Rec::at(pictures, delay as usize)
            } else {
                None
            }?;
            read_blip(&rec)
        })
        .collect()
}

fn read_blip(r: &Rec) -> Option<Blip> {
    let two_uids = r.inst & 1 == 1;
    let uids = if two_uids { 32 } else { 16 };
    let mut c = r.cur();
    let (ext, metafile, renderable) = match r.ty {
        0xF01A => ("emf", true, false),
        0xF01B => ("wmf", true, false),
        0xF01C => ("pict", true, false),
        0xF01D | 0xF02A => ("jpeg", false, true),
        0xF01E => ("png", false, true),
        0xF01F => ("bmp", false, true),
        0xF029 => ("tiff", false, false),
        _ => return None,
    };
    c.skip(uids)?;
    if metafile {
        let raw_size = c.u32()? as usize;
        c.skip(16 + 8)?;
        let saved = c.u32()? as usize;
        let compression = c.u8()?;
        c.skip(1)?;
        let data = c.bytes(saved.min(c.rest().len()))?;
        let data = if compression == 0 {
            let mut out = Vec::new();
            flate2::read::ZlibDecoder::new(data)
                .take(raw_size.min(256 << 20) as u64)
                .read_to_end(&mut out)
                .ok()?;
            out
        } else {
            data.to_vec()
        };
        return Some(Blip {
            ext,
            data,
            renderable,
        });
    }
    c.skip(1)?; // tag
    let data = c.rest();
    let data = if ext == "bmp" {
        dib_to_bmp(data)?
    } else {
        data.to_vec()
    };
    Some(Blip {
        ext,
        data,
        renderable,
    })
}

/// A DIB as a `.bmp` file (adds the file header).
fn dib_to_bmp(dib: &[u8]) -> Option<Vec<u8>> {
    let mut c = Cur::new(dib);
    let header = c.u32()?;
    let (bits, used) = if header >= 40 {
        let mut c = Cur::new(dib);
        c.skip(14)?;
        let bits = c.u16()?;
        c.skip(16)?;
        (bits, c.u32()?)
    } else {
        (24, 0)
    };
    let colors = if used > 0 {
        used.min(256)
    } else if bits <= 8 {
        1 << bits
    } else {
        0
    };
    let offset = 14 + header + colors * 4;
    let total = 14 + dib.len() as u32;
    let mut v = Vec::with_capacity(total as usize);
    v.extend_from_slice(b"BM");
    v.extend_from_slice(&total.to_le_bytes());
    v.extend_from_slice(&[0; 4]);
    v.extend_from_slice(&offset.to_le_bytes());
    v.extend_from_slice(dib);
    Some(v)
}

/// [`convert`] from the three streams (for tests that damage them).
#[doc(hidden)]
pub fn convert_streams(doc: &[u8], cur_user: &[u8], pictures: &[u8]) -> anyhow::Result<Imported> {
    // Current User → the last edit.
    let cu = Rec::at(cur_user, 0)
        .filter(|r| r.ty == rt::CURRENT_USER)
        .ok_or_else(old_version)?;
    let mut c = cu.cur();
    let _size = c.u32();
    let token = c.u32().ok_or_else(damaged)?;
    let edit = c.u32().ok_or_else(damaged)?;
    let _name_len = c.u16();
    let version = c.u16();
    let major = c.u8();
    if token == 0xF3D1_C4DF {
        return Err(encrypted());
    }
    if version.is_some_and(|v| v != 0x03F4) || major.is_some_and(|m| m < 3) {
        return Err(old_version());
    }
    let (persist, doc_ref) = persist_map(doc, edit)?;
    let at = |id: u32| persist.get(&id).and_then(|&o| Rec::at(doc, o as usize));
    let document = at(doc_ref)
        .filter(|r| r.ty == rt::DOCUMENT)
        .ok_or_else(damaged)?;
    if document.find(rt::CRYPT_SESSION).is_some() {
        return Err(encrypted());
    }

    let mut c = document.child(rt::DOCUMENT_ATOM).ok_or_else(damaged)?.cur();
    let (sw, sh) = (c.i32().unwrap_or(5760), c.i32().unwrap_or(4320));
    let (nw, nh) = (c.i32().unwrap_or(4320), c.i32().unwrap_or(5760));
    let size_emu = |v: i32, def: i64| {
        let e = (v as f64 * MU_EMU).round() as i64;
        if (914_400..=51_206_400).contains(&e) {
            e
        } else {
            def
        }
    };
    let slide_size = (size_emu(sw, 9_144_000), size_emu(sh, 6_858_000));
    let notes_size = (size_emu(nw, 6_858_000), size_emu(nh, 9_144_000));

    let env = document.child(rt::ENVIRONMENT);
    let fonts = font_names(env.as_ref());
    let doc_styles = env.map(|e| Styles::read(&e)).unwrap_or_default();
    let dgg = document
        .child(rt::DRAWING_GROUP)
        .and_then(|d| d.child(rt::DGG_CONTAINER));
    let mut dgg_defaults = Props::default();
    if let Some(d) = &dgg
        && let Some(f) = d.child(rt::FOPT)
    {
        dgg_defaults.read(&f);
    }
    let blips = read_blips(dgg.as_ref(), pictures);

    // Master, slide and notes lists.
    let lists: Vec<Rec> = document.kids(rt::SLIDE_LIST_WITH_TEXT).collect();
    let list = |inst: u16| {
        lists
            .iter()
            .find(|l| l.inst == inst)
            .map(slide_list)
            .unwrap_or_default()
    };
    let mut masters = Vec::new();
    for (pid, sid, _) in list(1) {
        let Some(r) = at(pid) else { continue };
        match r.ty {
            rt::MAIN_MASTER => masters.push(MasterSrc {
                id: sid,
                rec: r,
                main: None,
                flags: slide_atom(&r).2,
            }),
            rt::SLIDE => {
                let (_, m, flags) = slide_atom(&r);
                masters.push(MasterSrc {
                    id: sid,
                    rec: r,
                    main: Some(m),
                    flags,
                })
            }
            _ => {}
        }
    }
    let mut slides = Vec::new();
    for (pid, sid, texts) in list(0) {
        let Some(r) = at(pid).filter(|r| r.ty == rt::SLIDE) else {
            continue;
        };
        let (geom, master, flags) = slide_atom(&r);
        let hidden = r
            .child(rt::SLIDE_SHOW_SLIDE_INFO)
            .and_then(|i| {
                let mut c = i.cur();
                c.skip(10)?;
                c.u16()
            })
            .is_some_and(|f| f & 0x4 != 0);
        slides.push(SlideSrc {
            id: sid,
            rec: r,
            texts,
            master,
            geom,
            flags,
            hidden,
        });
    }
    let mut notes_by_slide: HashMap<u32, Rec> = HashMap::new();
    for (pid, _, _) in list(2) {
        if let Some(r) = at(pid).filter(|r| r.ty == rt::NOTES)
            && let Some(a) = r.child(rt::NOTES_ATOM)
            && let Some(sid) = a.cur().u32()
        {
            notes_by_slide.entry(sid).or_insert(r);
        }
    }

    // Master shape properties, for slide shapes that point at them.
    let mut master_props = HashMap::new();
    let master_shapes: Vec<(Vec<Shape>, Option<Shape>)> =
        masters.iter().map(|m| draw::read_drawing(&m.rec)).collect();
    for (shapes, _) in &master_shapes {
        draw::index_props(shapes, &mut master_props);
    }

    let doc_info = Doc {
        fonts,
        styles: doc_styles,
        dgg_defaults,
        master_props,
        blips,
    };
    let pkg = Package {
        doc: &doc_info,
        masters: &masters,
        master_shapes: &master_shapes,
        slides: &slides,
        notes: &notes_by_slide,
        slide_size,
        notes_size,
    };
    pkg.write()
}

const NS: &str = r#"xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main""#;
const DECL: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n";
const REL: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const CT: &str = "application/vnd.openxmlformats-officedocument.presentationml";
const GRP: &str = r#"<p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="0" cy="0"/><a:chOff x="0" y="0"/><a:chExt cx="0" cy="0"/></a:xfrm></p:grpSpPr>"#;
const CLR_MAP: &str = r#"<p:clrMap bg1="lt1" tx1="dk1" bg2="lt2" tx2="dk2" accent1="accent1" accent2="accent2" accent3="accent3" accent4="accent4" accent5="accent5" accent6="accent6" hlink="hlink" folHlink="folHlink"/>"#;

fn rels_xml(rels: &[(&str, String)]) -> String {
    let mut s = format!(
        "{DECL}<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">"
    );
    for (i, (ty, target)) in rels.iter().enumerate() {
        s.push_str(&format!(
            "<Relationship Id=\"rId{}\" Type=\"{REL}/{ty}\" Target=\"{}\"/>",
            i + 1,
            esc(target)
        ));
    }
    s.push_str("</Relationships>");
    s
}

fn hex(c: [u8; 3]) -> String {
    format!("{:02X}{:02X}{:02X}", c[0], c[1], c[2])
}

fn theme_xml(scheme: &[[u8; 3]; 8], major: &str, minor: &str) -> String {
    let clr =
        |name: &str, c: [u8; 3]| format!("<a:{name}><a:srgbClr val=\"{}\"/></a:{name}>", hex(c));
    let fill = "<a:solidFill><a:schemeClr val=\"phClr\"/></a:solidFill>";
    let line = |w: u32| {
        format!(
            "<a:ln w=\"{w}\" cap=\"flat\" cmpd=\"sng\" algn=\"ctr\"><a:solidFill><a:schemeClr val=\"phClr\"/></a:solidFill><a:prstDash val=\"solid\"/></a:ln>"
        )
    };
    let effect = "<a:effectStyle><a:effectLst/></a:effectStyle>";
    format!(
        "{DECL}<a:theme xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\" name=\"Imported\"><a:themeElements>\
         <a:clrScheme name=\"Imported\">{}{}{}{}{}{}{}{}{}{}{}{}</a:clrScheme>\
         <a:fontScheme name=\"Imported\"><a:majorFont><a:latin typeface=\"{}\"/><a:ea typeface=\"\"/><a:cs typeface=\"\"/></a:majorFont>\
         <a:minorFont><a:latin typeface=\"{}\"/><a:ea typeface=\"\"/><a:cs typeface=\"\"/></a:minorFont></a:fontScheme>\
         <a:fmtScheme name=\"Imported\"><a:fillStyleLst>{fill}{fill}{fill}</a:fillStyleLst>\
         <a:lnStyleLst>{}{}{}</a:lnStyleLst><a:effectStyleLst>{effect}{effect}{effect}</a:effectStyleLst>\
         <a:bgFillStyleLst>{fill}{fill}{fill}</a:bgFillStyleLst></a:fmtScheme></a:themeElements>\
         <a:objectDefaults/><a:extraClrSchemeLst/></a:theme>",
        clr("dk1", scheme[1]),
        clr("lt1", scheme[0]),
        clr("dk2", scheme[3]),
        clr("lt2", scheme[2]),
        clr("accent1", scheme[4]),
        clr("accent2", scheme[5]),
        clr("accent3", scheme[6]),
        clr("accent4", scheme[7]),
        clr("accent5", [0x5B, 0x9B, 0xD5]),
        clr("accent6", [0x70, 0xAD, 0x47]),
        clr("hlink", scheme[6]),
        clr("folHlink", scheme[7]),
        esc(major),
        esc(minor),
        line(6350),
        line(12700),
        line(19050),
    )
}

struct Package<'p, 'a> {
    doc: &'p Doc<'a>,
    masters: &'p [MasterSrc<'a>],
    master_shapes: &'p [(Vec<Shape<'a>>, Option<Shape<'a>>)],
    slides: &'p [SlideSrc<'a>],
    notes: &'p HashMap<u32, Rec<'a>>,
    slide_size: (i64, i64),
    notes_size: (i64, i64),
}

/// A master written to the package: its main master, the title master
/// that becomes its "Title Slide" layout, and its styles.
struct OutMaster {
    src: Option<usize>,
    title: Option<usize>,
    styles: Styles,
    scheme: [[u8; 3]; 8],
}

impl OutMaster {
    /// Layouts: Title Slide, Title and Content, Blank.
    const LAYOUTS: usize = 3;
}

impl<'a> Package<'_, 'a> {
    fn write(&self) -> anyhow::Result<Imported> {
        let doc = self.doc;
        // Masters to write: those slides use, else the first.
        let main_of = |id: u32| -> Option<usize> {
            let i = self.masters.iter().position(|m| m.id == id)?;
            match self.masters[i].main {
                None => Some(i),
                Some(main) => self
                    .masters
                    .iter()
                    .position(|m| m.id == main && m.main.is_none()),
            }
        };
        let mut used: Vec<usize> = Vec::new();
        for s in self.slides {
            if let Some(m) = main_of(s.master)
                && !used.contains(&m)
            {
                used.push(m);
            }
        }
        if used.is_empty()
            && let Some(m) = self.masters.iter().position(|m| m.main.is_none())
        {
            used.push(m);
        }
        used.sort_unstable();
        let mut out: Vec<OutMaster> = used
            .iter()
            .map(|&i| {
                let r = &self.masters[i].rec;
                OutMaster {
                    src: Some(i),
                    title: self
                        .masters
                        .iter()
                        .position(|t| t.main.is_some_and(|m| m == self.masters[i].id)),
                    styles: Styles::read(r),
                    scheme: scheme_of(r).unwrap_or(DEFAULT_SCHEME),
                }
            })
            .collect();
        if out.is_empty() {
            out.push(OutMaster {
                src: None,
                title: None,
                styles: Styles::default(),
                scheme: DEFAULT_SCHEME,
            });
        }

        let mut parts: Vec<(String, Vec<u8>)> = Vec::new();
        let mut overrides: Vec<(String, String)> = Vec::new();
        let mut media = Media::default();
        let mut pres_rels: Vec<(&str, String)> = Vec::new();
        let mut master_ids = String::new();
        let mut next_layout_id: u64 = 2_147_483_648 + out.len() as u64;

        for (mi, m) in out.iter().enumerate() {
            let n = mi + 1;
            let cx = Ctx {
                fonts: &doc.fonts,
                scheme: m.scheme,
            };
            let font_of = |ty: u32| {
                let (_, cf) = m.styles.resolve(&doc.styles, ty, 0);
                cf.font
                    .and_then(|f| cx.font(f))
                    .unwrap_or_else(|| "Arial".into())
            };
            parts.push((
                format!("ppt/theme/theme{n}.xml"),
                theme_xml(&m.scheme, &font_of(tx::TITLE), &font_of(tx::BODY)).into_bytes(),
            ));
            overrides.push((
                format!("/ppt/theme/theme{n}.xml"),
                "application/vnd.openxmlformats-officedocument.theme+xml".into(),
            ));

            // The master part.
            let mut part = Part {
                doc,
                cx,
                styles: &m.styles,
                rels: Vec::new(),
                media: &mut media,
                next_id: 1,
                outline: &[],
                ph_inherit: true,
            };
            for l in 0..OutMaster::LAYOUTS {
                part.rel(
                    "slideLayout",
                    format!(
                        "../slideLayouts/slideLayout{}.xml",
                        mi * OutMaster::LAYOUTS + l + 1
                    ),
                );
            }
            part.rel("theme", format!("../theme/theme{n}.xml"));
            let (shapes, bg) = match m.src {
                Some(i) => (
                    &self.master_shapes[i].0[..],
                    self.master_shapes[i].1.as_ref(),
                ),
                None => (&[][..], None),
            };
            let bg_xml = bg
                .map(|b| part.background(b))
                .filter(|b| !b.is_empty())
                .unwrap_or_else(|| {
                    format!(
                        "<p:bg><p:bgPr><a:solidFill><a:srgbClr val=\"{}\"/></a:solidFill><a:effectLst/></p:bgPr></p:bg>",
                        hex(m.scheme[0])
                    )
                });
            let shapes_xml: String = shapes.iter().map(|s| part.shape(s, true)).collect();
            let style = |ty: u32| {
                let levels: Vec<text::Level> = (0..5)
                    .map(|l| m.styles.resolve(&doc.styles, ty, l))
                    .collect();
                text::list_style(&cx, &levels)
            };
            let mut layout_ids = String::new();
            for l in 0..OutMaster::LAYOUTS {
                layout_ids.push_str(&format!(
                    "<p:sldLayoutId id=\"{}\" r:id=\"rId{}\"/>",
                    next_layout_id,
                    l + 1
                ));
                next_layout_id += 1;
            }
            let master_xml = format!(
                "{DECL}<p:sldMaster {NS}><p:cSld>{bg_xml}<p:spTree>{GRP}{shapes_xml}</p:spTree></p:cSld>{CLR_MAP}\
                 <p:sldLayoutIdLst>{layout_ids}</p:sldLayoutIdLst>\
                 <p:txStyles><p:titleStyle>{}</p:titleStyle><p:bodyStyle>{}</p:bodyStyle><p:otherStyle>{}</p:otherStyle></p:txStyles></p:sldMaster>",
                style(tx::TITLE),
                style(tx::BODY),
                style(tx::OTHER),
            );
            let mrels = std::mem::take(&mut part.rels);
            drop(part);
            parts.push((
                format!("ppt/slideMasters/slideMaster{n}.xml"),
                master_xml.into_bytes(),
            ));
            parts.push((
                format!("ppt/slideMasters/_rels/slideMaster{n}.xml.rels"),
                rels_xml(&mrels).into_bytes(),
            ));
            overrides.push((
                format!("/ppt/slideMasters/slideMaster{n}.xml"),
                format!("{CT}.slideMaster+xml"),
            ));
            pres_rels.push(("slideMaster", format!("slideMasters/slideMaster{n}.xml")));
            master_ids.push_str(&format!(
                "<p:sldMasterId id=\"{}\" r:id=\"rId{}\"/>",
                2_147_483_648u64 + mi as u64,
                pres_rels.len()
            ));

            // Layouts.
            for l in 0..OutMaster::LAYOUTS {
                let ln = mi * OutMaster::LAYOUTS + l + 1;
                let mut part = Part {
                    doc,
                    cx,
                    styles: &m.styles,
                    rels: vec![("slideMaster", format!("../slideMasters/slideMaster{n}.xml"))],
                    media: &mut media,
                    next_id: 1,
                    outline: &[],
                    ph_inherit: true,
                };
                let (name, ty, body, show_master, bg) = match l {
                    0 => {
                        let (shapes, bg, show) = match m.title {
                            Some(t) => {
                                let src = &self.masters[t];
                                let (sh, bg) = &self.master_shapes[t];
                                let bg = (src.flags & 0x4 == 0)
                                    .then(|| bg.as_ref().map(|b| part.background(b)))
                                    .flatten()
                                    .unwrap_or_default();
                                let shapes: String =
                                    sh.iter().map(|s| part.shape(s, true)).collect();
                                (shapes, bg, src.flags & 0x1 != 0)
                            }
                            None => (self.title_placeholders(&cx, m), String::new(), true),
                        };
                        ("Title Slide", "title", shapes, show, bg)
                    }
                    1 => (
                        "Title and Content",
                        "obj",
                        layout_ph(2, "Title 1", "type=\"title\"", "")
                            + &layout_ph(3, "Content Placeholder 2", "idx=\"1\"", ""),
                        true,
                        String::new(),
                    ),
                    _ => ("Blank", "blank", String::new(), true, String::new()),
                };
                let show = if show_master {
                    ""
                } else {
                    " showMasterSp=\"0\""
                };
                let xml = format!(
                    "{DECL}<p:sldLayout {NS} type=\"{ty}\" preserve=\"1\"{show}><p:cSld name=\"{name}\">{bg}<p:spTree>{GRP}{body}</p:spTree></p:cSld>\
                     <p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr></p:sldLayout>"
                );
                let lrels = std::mem::take(&mut part.rels);
                drop(part);
                parts.push((
                    format!("ppt/slideLayouts/slideLayout{ln}.xml"),
                    xml.into_bytes(),
                ));
                parts.push((
                    format!("ppt/slideLayouts/_rels/slideLayout{ln}.xml.rels"),
                    rels_xml(&lrels).into_bytes(),
                ));
                overrides.push((
                    format!("/ppt/slideLayouts/slideLayout{ln}.xml"),
                    format!("{CT}.slideLayout+xml"),
                ));
            }
        }

        // Notes master, if any slide has notes.
        let has_notes = self.slides.iter().any(|s| self.notes.contains_key(&s.id));
        let mut notes_master_rel = None;
        if has_notes {
            let tn = out.len() + 1;
            parts.push((
                format!("ppt/theme/theme{tn}.xml"),
                theme_xml(&out[0].scheme, "Arial", "Arial").into_bytes(),
            ));
            overrides.push((
                format!("/ppt/theme/theme{tn}.xml"),
                "application/vnd.openxmlformats-officedocument.theme+xml".into(),
            ));
            parts.push((
                "ppt/notesMasters/notesMaster1.xml".into(),
                crate::template::notes_master().into_bytes(),
            ));
            parts.push((
                "ppt/notesMasters/_rels/notesMaster1.xml.rels".into(),
                rels_xml(&[("theme", format!("../theme/theme{tn}.xml"))]).into_bytes(),
            ));
            overrides.push((
                "/ppt/notesMasters/notesMaster1.xml".into(),
                format!("{CT}.notesMaster+xml"),
            ));
            pres_rels.push(("notesMaster", "notesMasters/notesMaster1.xml".into()));
            notes_master_rel = Some(pres_rels.len());
        }

        // Slides.
        let mut slide_ids = String::new();
        let mut notes_count = 0usize;
        for (si, s) in self.slides.iter().enumerate() {
            let n = si + 1;
            let main = main_of(s.master);
            let mi = main
                .and_then(|i| out.iter().position(|o| o.src == Some(i)))
                .unwrap_or(0);
            let is_title_master = self
                .masters
                .iter()
                .any(|m| m.id == s.master && m.main.is_some());
            let layout = if is_title_master || s.geom == 0 {
                0
            } else if s.geom == 16 {
                2
            } else {
                1
            };
            let m = &out[mi];
            let scheme = if s.flags & 0x2 != 0 {
                m.scheme
            } else {
                scheme_of(&s.rec).unwrap_or(m.scheme)
            };
            let cx = Ctx {
                fonts: &doc.fonts,
                scheme,
            };
            let mut part = Part {
                doc,
                cx,
                styles: &m.styles,
                rels: vec![(
                    "slideLayout",
                    format!(
                        "../slideLayouts/slideLayout{}.xml",
                        mi * OutMaster::LAYOUTS + layout + 1
                    ),
                )],
                media: &mut media,
                next_id: 1,
                outline: &s.texts,
                ph_inherit: true,
            };
            let (shapes, bg) = draw::read_drawing(&s.rec);
            let bg_xml = if s.flags & 0x4 == 0 {
                bg.as_ref().map(|b| part.background(b)).unwrap_or_default()
            } else {
                String::new()
            };
            let shapes_xml: String = shapes.iter().map(|sh| part.shape(sh, true)).collect();
            if let Some(nr) = self.notes.get(&s.id) {
                notes_count += 1;
                let nn = notes_count;
                part.rel("notesSlide", format!("../notesSlides/notesSlide{nn}.xml"));
                let ncx = cx;
                let body = notes_text(nr)
                    .map(|t| {
                        let styles = &m.styles;
                        let style = |l: usize| styles.resolve(&doc.styles, tx::NOTES, l);
                        // Notes keep their characters and bold/italic; the
                        // notes master gives the size.
                        let mut t = t;
                        for (_, cf) in &mut t.cf_runs {
                            cf.size = None;
                            cf.font = None;
                        }
                        text::paragraphs(&ncx, &t, &style)
                    })
                    .unwrap_or_else(|| "<a:p><a:endParaRPr lang=\"en-US\"/></a:p>".into());
                let xml = crate::template::notes_slide()
                    .replace("<a:p><a:endParaRPr lang=\"en-US\"/></a:p>", &body);
                parts.push((
                    format!("ppt/notesSlides/notesSlide{nn}.xml"),
                    xml.into_bytes(),
                ));
                parts.push((
                    format!("ppt/notesSlides/_rels/notesSlide{nn}.xml.rels"),
                    rels_xml(&[
                        ("notesMaster", "../notesMasters/notesMaster1.xml".into()),
                        ("slide", format!("../slides/slide{n}.xml")),
                    ])
                    .into_bytes(),
                ));
                overrides.push((
                    format!("/ppt/notesSlides/notesSlide{nn}.xml"),
                    format!("{CT}.notesSlide+xml"),
                ));
            }
            let mut attrs = String::new();
            if s.flags & 0x1 == 0 {
                attrs.push_str(" showMasterSp=\"0\"");
            }
            if s.hidden {
                attrs.push_str(" show=\"0\"");
            }
            let xml = format!(
                "{DECL}<p:sld {NS}{attrs}><p:cSld>{bg_xml}<p:spTree>{GRP}{shapes_xml}</p:spTree></p:cSld>\
                 <p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr></p:sld>"
            );
            let srels = std::mem::take(&mut part.rels);
            drop(part);
            parts.push((format!("ppt/slides/slide{n}.xml"), xml.into_bytes()));
            parts.push((
                format!("ppt/slides/_rels/slide{n}.xml.rels"),
                rels_xml(&srels).into_bytes(),
            ));
            overrides.push((
                format!("/ppt/slides/slide{n}.xml"),
                format!("{CT}.slide+xml"),
            ));
            pres_rels.push(("slide", format!("slides/slide{n}.xml")));
            slide_ids.push_str(&format!(
                "<p:sldId id=\"{}\" r:id=\"rId{}\"/>",
                256 + si,
                pres_rels.len()
            ));
        }

        // Presentation.
        let first = &out[0];
        let cx = Ctx {
            fonts: &doc.fonts,
            scheme: first.scheme,
        };
        let default_style = {
            let levels: Vec<text::Level> = (0..5)
                .map(|l| doc.styles.resolve(&doc.styles, tx::OTHER, l))
                .collect();
            text::list_style(&cx, &levels)
        };
        for (ty, target) in [
            ("presProps", "presProps.xml"),
            ("viewProps", "viewProps.xml"),
            ("tableStyles", "tableStyles.xml"),
        ] {
            pres_rels.push((ty, target.into()));
        }
        let notes_ids = notes_master_rel
            .map(|r| {
                format!(
                    "<p:notesMasterIdLst><p:notesMasterId r:id=\"rId{r}\"/></p:notesMasterIdLst>"
                )
            })
            .unwrap_or_default();
        let sld_list = if slide_ids.is_empty() {
            String::new()
        } else {
            format!("<p:sldIdLst>{slide_ids}</p:sldIdLst>")
        };
        let presentation = format!(
            "{DECL}<p:presentation {NS} saveSubsetFonts=\"1\"><p:sldMasterIdLst>{master_ids}</p:sldMasterIdLst>{notes_ids}{sld_list}\
             <p:sldSz cx=\"{}\" cy=\"{}\"/><p:notesSz cx=\"{}\" cy=\"{}\"/>\
             <p:defaultTextStyle><a:defPPr><a:defRPr lang=\"en-US\"/></a:defPPr>{default_style}</p:defaultTextStyle></p:presentation>",
            self.slide_size.0, self.slide_size.1, self.notes_size.0, self.notes_size.1
        );
        parts.push(("ppt/presentation.xml".into(), presentation.into_bytes()));
        parts.push((
            "ppt/_rels/presentation.xml.rels".into(),
            rels_xml(&pres_rels).into_bytes(),
        ));
        parts.push((
            "ppt/presProps.xml".into(),
            format!("{DECL}<p:presentationPr {NS}/>").into_bytes(),
        ));
        parts.push((
            "ppt/viewProps.xml".into(),
            format!("{DECL}<p:viewPr {NS}><p:gridSpacing cx=\"76200\" cy=\"76200\"/></p:viewPr>")
                .into_bytes(),
        ));
        parts.push((
            "ppt/tableStyles.xml".into(),
            format!(
                "{DECL}<a:tblStyleLst xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\" def=\"{{5C22544A-7EE6-4342-B048-85BDC9FD1C3A}}\"/>"
            )
            .into_bytes(),
        ));
        for (name, data) in std::mem::take(&mut media.parts) {
            parts.push((format!("ppt/media/{name}"), data));
        }
        let mut ct = format!(
            "{DECL}<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">\
             <Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/>\
             <Default Extension=\"xml\" ContentType=\"application/xml\"/>\
             <Default Extension=\"png\" ContentType=\"image/png\"/>\
             <Default Extension=\"jpeg\" ContentType=\"image/jpeg\"/>\
             <Default Extension=\"bmp\" ContentType=\"image/bmp\"/>\
             <Default Extension=\"tiff\" ContentType=\"image/tiff\"/>\
             <Default Extension=\"emf\" ContentType=\"image/x-emf\"/>\
             <Default Extension=\"wmf\" ContentType=\"image/x-wmf\"/>\
             <Default Extension=\"pict\" ContentType=\"image/pict\"/>\
             <Override PartName=\"/ppt/presentation.xml\" ContentType=\"{CT}.presentation.main+xml\"/>\
             <Override PartName=\"/ppt/presProps.xml\" ContentType=\"{CT}.presProps+xml\"/>\
             <Override PartName=\"/ppt/viewProps.xml\" ContentType=\"{CT}.viewProps+xml\"/>\
             <Override PartName=\"/ppt/tableStyles.xml\" ContentType=\"{CT}.tableStyles+xml\"/>\
             <Override PartName=\"/docProps/core.xml\" ContentType=\"application/vnd.openxmlformats-package.core-properties+xml\"/>\
             <Override PartName=\"/docProps/app.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.extended-properties+xml\"/>"
        );
        for (name, ty) in &overrides {
            ct.push_str(&format!(
                "<Override PartName=\"{name}\" ContentType=\"{ty}\"/>"
            ));
        }
        ct.push_str("</Types>");
        parts.push(("[Content_Types].xml".into(), ct.into_bytes()));
        parts.push((
            "_rels/.rels".into(),
            format!(
                "{DECL}<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
                 <Relationship Id=\"rId1\" Type=\"{REL}/officeDocument\" Target=\"ppt/presentation.xml\"/>\
                 <Relationship Id=\"rId2\" Type=\"http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties\" Target=\"docProps/core.xml\"/>\
                 <Relationship Id=\"rId3\" Type=\"{REL}/extended-properties\" Target=\"docProps/app.xml\"/></Relationships>"
            )
            .into_bytes(),
        ));
        parts.push((
            "docProps/core.xml".into(),
            format!(
                "{DECL}<cp:coreProperties xmlns:cp=\"http://schemas.openxmlformats.org/package/2006/metadata/core-properties\" \
                 xmlns:dc=\"http://purl.org/dc/elements/1.1/\" xmlns:dcterms=\"http://purl.org/dc/terms/\" \
                 xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\"><dc:title></dc:title></cp:coreProperties>"
            )
            .into_bytes(),
        ));
        parts.push((
            "docProps/app.xml".into(),
            format!(
                "{DECL}<Properties xmlns=\"http://schemas.openxmlformats.org/officeDocument/2006/extended-properties\"><Application>Kantor</Application></Properties>"
            )
            .into_bytes(),
        ));

        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        for (name, data) in parts {
            zip.start_file(name, opts).context("writing the package")?;
            zip.write_all(&data).context("writing the package")?;
        }
        let pptx = zip.finish().context("writing the package")?.into_inner();
        let mut notes = Vec::new();
        if media.hidden > 0 {
            notes.push(format!(
                "{} EMF/WMF picture{} shown as {}",
                media.hidden,
                if media.hidden == 1 { "" } else { "s" },
                if media.hidden == 1 { "a box" } else { "boxes" }
            ));
        }
        Ok(Imported { pptx, notes })
    }

    /// Centred title and subtitle placeholders for a master without a
    /// title master.
    fn title_placeholders(&self, cx: &Ctx, m: &OutMaster) -> String {
        let (w, h) = self.slide_size;
        let lst = |ty: u32| {
            let levels: Vec<text::Level> = (0..5)
                .map(|l| m.styles.resolve(&self.doc.styles, ty, l))
                .collect();
            text::list_style(cx, &levels)
        };
        let xf = |x: f64, y: f64, cw: f64, ch: f64| {
            format!(
                "<a:xfrm><a:off x=\"{}\" y=\"{}\"/><a:ext cx=\"{}\" cy=\"{}\"/></a:xfrm>",
                (w as f64 * x) as i64,
                (h as f64 * y) as i64,
                (w as f64 * cw) as i64,
                (h as f64 * ch) as i64
            )
        };
        layout_ph(
            2,
            "Title 1",
            "type=\"ctrTitle\"",
            &format!(
                "<p:spPr>{}</p:spPr><p:txBody><a:bodyPr anchor=\"b\"/><a:lstStyle>{}</a:lstStyle><a:p><a:endParaRPr lang=\"en-US\"/></a:p></p:txBody>",
                xf(0.125, 0.16, 0.75, 0.35),
                lst(tx::CENTER_TITLE)
            ),
        ) + &layout_ph(
            3,
            "Subtitle 2",
            "type=\"subTitle\" idx=\"1\"",
            &format!(
                "<p:spPr>{}</p:spPr><p:txBody><a:bodyPr/><a:lstStyle>{}</a:lstStyle><a:p><a:endParaRPr lang=\"en-US\"/></a:p></p:txBody>",
                xf(0.125, 0.53, 0.75, 0.24),
                lst(tx::CENTER_BODY)
            ),
        )
    }
}

/// A layout placeholder; `inner` is its `p:spPr` and `p:txBody` (empty for
/// "inherit everything from the master").
fn layout_ph(id: u32, name: &str, ph: &str, inner: &str) -> String {
    let inner = if inner.is_empty() {
        "<p:spPr/><p:txBody><a:bodyPr/><a:lstStyle/><a:p><a:endParaRPr lang=\"en-US\"/></a:p></p:txBody>"
    } else {
        inner
    };
    format!(
        "<p:sp><p:nvSpPr><p:cNvPr id=\"{id}\" name=\"{name}\"/><p:cNvSpPr><a:spLocks noGrp=\"1\"/></p:cNvSpPr><p:nvPr><p:ph {ph}/></p:nvPr></p:nvSpPr>{inner}</p:sp>"
    )
}

/// The notes text of a NotesContainer: its notes body placeholder.
fn notes_text(r: &Rec) -> Option<Text> {
    let (shapes, _) = draw::read_drawing(r);
    fn find(shapes: &[Shape]) -> Option<Text> {
        for s in shapes {
            if let Some(t) = &s.text
                && (matches!(s.ph, Some((12 | 6, _))) || t.ty == tx::NOTES)
            {
                return Some(t.clone());
            }
            if let Some(t) = find(&s.kids) {
                return Some(t);
            }
        }
        None
    }
    find(&shapes)
}
