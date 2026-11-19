//! Static fonts made from one instance of a variable font.
//!
//! When the only face of a family is a variable font, fontique picks it for
//! bold or italic text too and parley shapes with the instance's axis
//! values (`wght` 700, `ital` 1, …), so line breaking already uses the
//! instance's advances. Renderers that only take a font file (GPUI's glyph
//! rasterizer, krilla's PDF fonts) would draw the default instance instead,
//! i.e. regular text where the document asks for bold. [`instance`] turns
//! the variable font into an ordinary static font at those axis values,
//! with the same glyph ids, so they draw the real bold or italic: the same
//! thing Word or PowerPoint do when only a variable font is installed.
//!
//! The instance is named the way the font names it (the named instance's
//! PostScript name, or `<variations prefix>-<subfamily>`), e.g.
//! `PlusJakartaSans-Bold`, which is what a PDF lists as the font.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use parley::FontData;
use skrifa::raw::{FontRef, TableProvider as _};
use skrifa::{MetadataProvider as _, Tag, string::StringId};

/// A static font cut from a variable font at one location.
#[derive(Debug)]
pub struct Instance {
    /// A complete sfnt: same glyph ids and `cmap` as the variable font,
    /// outlines and advances at the instance's location.
    pub data: Arc<Vec<u8>>,
    /// The instance's own family name (unique to it, so it can be
    /// registered with a text system beside the variable font).
    pub family: String,
    /// e.g. `PlusJakartaSans-Bold`.
    pub postscript_name: String,
    /// e.g. `Bold`.
    pub subfamily: String,
    pub weight: f32,
    pub italic: bool,
}

/// A font face at a location: font data id, face index and axis values
/// (as bits, so it can be hashed).
pub type Key = (u64, u32, Vec<([u8; 4], u32)>);

/// The cache key of `font` at `variations`.
pub fn key(font: &FontData, variations: &[([u8; 4], f32)]) -> Key {
    (
        font.data.id(),
        font.index,
        variations.iter().map(|(t, v)| (*t, v.to_bits())).collect(),
    )
}

fn cache() -> &'static Mutex<HashMap<Key, Option<Arc<Instance>>>> {
    static CACHE: OnceLock<Mutex<HashMap<Key, Option<Arc<Instance>>>>> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

/// The static instance of `font` at `variations` (user-space axis values,
/// as fontique's synthesis gives them). `None` when the font isn't
/// variable, the values are its default location, or it can't be
/// instanced. Made once per font and location, then shared.
pub fn instance(font: &FontData, variations: &[([u8; 4], f32)]) -> Option<Arc<Instance>> {
    if variations.is_empty() {
        return None;
    }
    let mut key = key(font, variations);
    key.2.sort_unstable();
    if let Some(hit) = cache().lock().ok()?.get(&key) {
        return hit.clone();
    }
    let made = make(font.data.data(), font.index, variations).map(Arc::new);
    if made.is_none() {
        log::debug!("no static instance for a variable font at {variations:?}");
    }
    cache().lock().ok()?.insert(key, made.clone());
    made
}

/// The variable font's axis values for `variations`, clamped to its axes;
/// `None` when they're all at the default.
fn location(f: &FontRef, variations: &[([u8; 4], f32)]) -> Option<Vec<(Tag, f32)>> {
    let axes = f.axes();
    if axes.is_empty() {
        return None;
    }
    let mut out = Vec::new();
    let mut moved = false;
    for axis in axes.iter() {
        let tag = axis.tag();
        let v = variations
            .iter()
            .rev()
            .find(|(t, _)| Tag::new(t) == tag)
            .map_or(axis.default_value(), |(_, v)| {
                // fontique asks for italic as `slnt` +14 (a CSS oblique
                // angle), but `slnt` counts counter-clockwise: text that
                // leans right, as italic does, is negative (Inter's axis
                // runs -10..0).
                let v = if tag == Tag::new(b"slnt") {
                    -v.abs()
                } else {
                    *v
                };
                v.clamp(axis.min_value(), axis.max_value())
            });
        moved |= (v - axis.default_value()).abs() > f32::EPSILON;
        out.push((tag, v));
    }
    moved.then_some(out)
}

fn name(f: &FontRef, id: StringId) -> Option<String> {
    f.localized_strings(id)
        .english_or_first()
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty())
}

/// Standard names for `wght` values without a named instance.
fn weight_name(w: f32) -> &'static str {
    match w.round() as i32 {
        ..=149 => "Thin",
        150..=249 => "ExtraLight",
        250..=349 => "Light",
        350..=449 => "Regular",
        450..=549 => "Medium",
        550..=649 => "SemiBold",
        650..=749 => "Bold",
        750..=849 => "ExtraBold",
        _ => "Black",
    }
}

fn make(data: &[u8], index: u32, variations: &[([u8; 4], f32)]) -> Option<Instance> {
    let f = FontRef::from_index(data, index).ok()?;
    let loc = location(&f, variations)?;
    let at = |tag: &[u8; 4]| {
        loc.iter()
            .find(|(t, _)| *t == Tag::new(tag))
            .map(|(_, v)| *v)
    };
    let old_attrs = f.attributes();
    let weight = at(b"wght").unwrap_or(old_attrs.weight.value());
    let italic = !matches!(old_attrs.style, skrifa::attribute::Style::Normal)
        || at(b"ital").is_some_and(|v| v >= 0.5)
        || at(b"slnt").is_some_and(|v| v != 0.0);

    // Names: the named instance at this location if there is one.
    let family =
        name(&f, StringId::TYPOGRAPHIC_FAMILY_NAME).or_else(|| name(&f, StringId::FAMILY_NAME))?;
    let named = f.named_instances().iter().find(|ni| {
        ni.user_coords()
            .zip(loc.iter())
            .all(|(a, (_, b))| (a - b).abs() < 0.5)
    });
    let subfamily = named
        .clone()
        .and_then(|ni| name(&f, ni.subfamily_name_id()))
        .unwrap_or_else(|| {
            let w = weight_name(weight);
            match (italic, w) {
                (true, "Regular") => "Italic".to_owned(),
                (true, w) => format!("{w} Italic"),
                (false, w) => w.to_owned(),
            }
        });
    let prefix = name(&f, StringId::VARIATIONS_POSTSCRIPT_NAME_PREFIX)
        .or_else(|| {
            name(&f, StringId::POSTSCRIPT_NAME)
                .map(|ps| ps.split('-').next().unwrap_or(&ps).to_owned())
        })
        .unwrap_or_else(|| family.clone());
    let postscript_name = named
        .and_then(|ni| ni.postscript_name_id())
        .and_then(|id| name(&f, id))
        .unwrap_or_else(|| format!("{prefix}-{subfamily}"))
        .chars()
        .filter(|c| c.is_ascii_graphic() && !"[](){}<>/%".contains(*c))
        .take(63)
        .collect::<String>();
    let instance_family = format!("{family} {subfamily} Instance");

    // Outlines and metrics at the location, every glyph kept in place.
    let n = f.maxp().ok()?.num_glyphs();
    let mut mapper = subsetter::GlyphRemapper::new();
    for gid in 0..n {
        if mapper.remap(gid) != gid {
            return None;
        }
    }
    let coords: Vec<(subsetter::Tag, f32)> = loc
        .iter()
        .map(|(t, v)| (subsetter::Tag::new(&t.to_be_bytes()), *v))
        .collect();
    let cut = subsetter::subset_with_variations(data, index, &coords, &mapper).ok()?;
    let cut_font = FontRef::new(&cut).ok()?;

    let mut tables: Vec<([u8; 4], Vec<u8>)> = Vec::new();
    for rec in cut_font.table_directory.table_records() {
        let tag = rec.tag().to_be_bytes();
        if &tag == b"name" {
            continue;
        }
        let mut bytes = cut_font.table_data(rec.tag())?.as_bytes().to_vec();
        if &tag == b"head" && bytes.len() >= 46 {
            // macStyle: bold, italic.
            let mut style = u16::from_be_bytes([bytes[44], bytes[45]]) & !0b11;
            style |= u16::from(weight >= 600.0) | (u16::from(italic) << 1);
            bytes[44..46].copy_from_slice(&style.to_be_bytes());
        }
        tables.push((tag, bytes));
    }
    // What text systems need besides the outlines: the character map and
    // OS/2 (weight class and style bits).
    if let Some(cmap) = f.table_data(Tag::new(b"cmap")) {
        tables.push((*b"cmap", cmap.as_bytes().to_vec()));
    }
    if let Some(os2) = f.table_data(Tag::new(b"OS/2")) {
        let mut os2 = os2.as_bytes().to_vec();
        if os2.len() >= 64 {
            let class = (weight.round().clamp(1.0, 1000.0)) as u16;
            os2[4..6].copy_from_slice(&class.to_be_bytes());
            if let Some(w) = at(b"wdth") {
                let class = match w {
                    ..56.25 => 1u16,
                    ..68.75 => 2,
                    ..81.25 => 3,
                    ..93.75 => 4,
                    ..106.25 => 5,
                    ..118.75 => 6,
                    ..137.5 => 7,
                    ..175.0 => 8,
                    _ => 9,
                };
                os2[6..8].copy_from_slice(&class.to_be_bytes());
            }
            // fsSelection: ITALIC (0), BOLD (5), REGULAR (6).
            let mut sel = u16::from_be_bytes([os2[62], os2[63]]) & !(1 | 1 << 5 | 1 << 6);
            let bold = weight >= 600.0;
            if italic {
                sel |= 1;
            }
            if bold {
                sel |= 1 << 5;
            }
            if !bold && !italic {
                sel |= 1 << 6;
            }
            os2[62..64].copy_from_slice(&sel.to_be_bytes());
        }
        tables.push((*b"OS/2", os2));
    }
    let copyright = name(&f, StringId::COPYRIGHT_NOTICE);
    let license = name(&f, StringId::LICENSE_DESCRIPTION);
    let license_url = name(&f, StringId::LICENSE_URL);
    let mut names: Vec<(u16, String)> = vec![
        (1, instance_family.clone()),
        (2, "Regular".to_owned()),
        (3, format!("{postscript_name};Kantor instance")),
        (4, format!("{family} {subfamily}")),
        (6, postscript_name.clone()),
    ];
    names.extend(copyright.map(|s| (0, s)));
    names.extend(license.map(|s| (13, s)));
    names.extend(license_url.map(|s| (14, s)));
    tables.push((*b"name", name_table(names)));
    Some(Instance {
        data: Arc::new(sfnt(tables)),
        family: instance_family,
        postscript_name,
        subfamily,
        weight,
        italic,
    })
}

/// A `name` table (format 0) with Windows Unicode English records.
fn name_table(mut names: Vec<(u16, String)>) -> Vec<u8> {
    names.sort_by_key(|(id, _)| *id);
    let mut strings: Vec<u8> = Vec::new();
    let mut out = Vec::new();
    out.extend(0u16.to_be_bytes());
    out.extend((names.len() as u16).to_be_bytes());
    out.extend((6 + 12 * names.len() as u16).to_be_bytes());
    for (id, s) in &names {
        let utf16: Vec<u8> = s.encode_utf16().flat_map(u16::to_be_bytes).collect();
        let len = utf16.len().min(u16::MAX as usize & !1);
        for v in [3u16, 1, 0x409, *id, len as u16, strings.len() as u16] {
            out.extend(v.to_be_bytes());
        }
        strings.extend(&utf16[..len]);
    }
    out.extend(strings);
    out
}

fn checksum(data: &[u8]) -> u32 {
    data.chunks(4).fold(0u32, |sum, c| {
        let mut b = [0u8; 4];
        b[..c.len()].copy_from_slice(c);
        sum.wrapping_add(u32::from_be_bytes(b))
    })
}

/// An sfnt file from its tables (sorted by tag, 4-byte aligned, with
/// checksums and `head.checksumAdjustment`).
fn sfnt(mut tables: Vec<([u8; 4], Vec<u8>)>) -> Vec<u8> {
    tables.sort_by_key(|(t, _)| *t);
    let glyf = tables.iter().any(|(t, _)| t == b"glyf");
    let n = tables.len() as u16;
    let sel = 15 - n.max(1).leading_zeros() as u16;
    let range = (1u16 << sel) * 16;
    let mut out = Vec::new();
    out.extend(
        if glyf {
            0x0001_0000u32
        } else {
            u32::from_be_bytes(*b"OTTO")
        }
        .to_be_bytes(),
    );
    for v in [n, range, sel, n * 16 - range] {
        out.extend(v.to_be_bytes());
    }
    let mut offset = 12 + 16 * tables.len();
    let mut head_at = None;
    for (tag, data) in &mut tables {
        if tag == b"head" && data.len() >= 12 {
            data[8..12].fill(0);
            head_at = Some(offset);
        }
        out.extend(*tag);
        out.extend(checksum(data).to_be_bytes());
        out.extend((offset as u32).to_be_bytes());
        out.extend((data.len() as u32).to_be_bytes());
        offset += data.len().next_multiple_of(4);
    }
    for (_, data) in &tables {
        out.extend(data);
        out.resize(out.len().next_multiple_of(4), 0);
    }
    if let Some(at) = head_at {
        let adj = 0xB1B0_AFBAu32.wrapping_sub(checksum(&out));
        out[at + 8..at + 12].copy_from_slice(&adj.to_be_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use parley::fontique::Blob;

    pub(crate) fn fixture() -> FontData {
        let bytes = include_bytes!("../../../tests/fonts/KantorVarTest.ttf").to_vec();
        FontData::new(Blob::new(Arc::new(bytes)), 0)
    }

    fn advance(data: &[u8], ch: char) -> u16 {
        let f = FontRef::new(data).unwrap();
        let gid = f.charmap().map(ch).unwrap();
        f.hmtx().unwrap().advance(gid).unwrap()
    }

    #[test]
    fn bold_instance_is_a_static_font_named_like_the_named_instance() {
        let font = fixture();
        let inst = instance(&font, &[(*b"wght", 700.0)]).expect("instance");
        assert_eq!(inst.postscript_name, "KantorVarTest-Bold");
        assert_eq!(inst.subfamily, "Bold");
        assert_eq!(inst.weight, 700.0);
        assert!(!inst.italic);
        let f = FontRef::new(&inst.data).unwrap();
        assert!(f.table_data(Tag::new(b"fvar")).is_none());
        assert!(f.table_data(Tag::new(b"gvar")).is_none());
        assert_eq!(f.attributes().weight.value(), 700.0);
        assert_eq!(
            name(&f, StringId::POSTSCRIPT_NAME).as_deref(),
            Some("KantorVarTest-Bold")
        );
        // Same glyph ids and character map; wider (bold) advances.
        let orig = FontRef::new(font.data.data()).unwrap();
        assert_eq!(
            f.maxp().unwrap().num_glyphs(),
            orig.maxp().unwrap().num_glyphs()
        );
        assert_eq!(f.charmap().map('m'), orig.charmap().map('m'));
        assert!(advance(&inst.data, 'm') > advance(font.data.data(), 'm'));
        // krilla reads it.
        assert!(krilla::text::Font::new(inst.data.to_vec().into(), 0).is_some());
        // Cached.
        let again = instance(&font, &[(*b"wght", 700.0)]).unwrap();
        assert!(Arc::ptr_eq(&inst, &again));
    }

    /// fontique picks the variable face for bold text and parley shapes
    /// it at `wght` 700: the run says so, and it's wider than regular.
    #[test]
    fn bold_text_in_a_variable_font_is_shaped_at_wght_700() {
        use crate::{Engine, PageSetup, Paragraph, Span};
        use parley::fontique::{Collection, CollectionOptions, SourceCache};
        let mut collection = Collection::new(CollectionOptions {
            shared: false,
            system_fonts: false,
        });
        collection.register_fonts(fixture().data.clone(), None);
        let fonts = parley::FontContext {
            collection,
            source_cache: SourceCache::default(),
        };
        let mut engine = Engine::with_fonts(fonts);
        let mut para = Paragraph::new("mmmm mmmm");
        para.family = "Kantor Var Test".into();
        para.spans = vec![Span {
            bold: true,
            ..Span::new(0..4)
        }];
        let doc = engine.layout_document(&[para], PageSetup::a4());
        let runs = doc.glyph_runs(0);
        let bold = runs.iter().find(|r| r.text_range.start == 0).unwrap();
        let regular = runs.iter().find(|r| r.text_range.start >= 4).unwrap();
        assert_eq!(bold.variations, vec![(*b"wght", 700.0)]);
        assert!(!bold.embolden, "real bold, not synthesized");
        assert!(regular.variations.is_empty());
        assert!(bold.glyphs[0].advance > regular.glyphs[1].advance);
        let inst = instance(&bold.font, &bold.variations).unwrap();
        assert_eq!(inst.postscript_name, "KantorVarTest-Bold");
    }

    fn engine() -> crate::Engine {
        use parley::fontique::{Collection, CollectionOptions, SourceCache};
        let mut collection = Collection::new(CollectionOptions {
            shared: false,
            system_fonts: false,
        });
        collection.register_fonts(fixture().data.clone(), None);
        crate::Engine::with_fonts(parley::FontContext {
            collection,
            source_cache: SourceCache::default(),
        })
    }

    /// "Kantor Var Test ExtraBold" (a style named as a family, as Windows
    /// lists a variable font's named instances) is the family at 800.
    #[test]
    fn a_style_named_as_a_family_is_that_weight_of_the_family() {
        use crate::{PageSetup, Paragraph, StyledFamily};
        let mut engine = engine();
        assert_eq!(engine.style_family("Kantor Var Test"), None);
        assert_eq!(engine.style_family("Nope Sans ExtraBold"), None);
        assert_eq!(
            engine.style_family("Kantor Var Test ExtraBold, Noto Sans"),
            Some(StyledFamily {
                family: "\"Kantor Var Test\", Noto Sans".into(),
                weight: 800.0,
                italic: false,
            })
        );
        assert_eq!(
            engine
                .style_family("Kantor Var Test Semi Bold Italic")
                .map(|s| (s.weight, s.italic)),
            Some((600.0, true))
        );
        let mut para = Paragraph::new("mmmm");
        para.family = "Kantor Var Test ExtraBold".into();
        let doc = engine.layout_document(&[para], PageSetup::a4());
        let runs = doc.glyph_runs(0);
        assert_eq!(runs[0].variations, vec![(*b"wght", 800.0)]);
        let ps = skrifa::FontRef::new(runs[0].font.data.data())
            .ok()
            .and_then(|f| name(&f, StringId::FAMILY_NAME));
        assert_eq!(ps.as_deref(), Some("Kantor Var Test"));
    }

    #[test]
    fn default_location_and_static_fonts_have_no_instance() {
        let font = fixture();
        assert!(instance(&font, &[]).is_none());
        assert!(instance(&font, &[(*b"wght", 400.0)]).is_none());
        // An axis the font doesn't have.
        assert!(instance(&font, &[(*b"ital", 1.0)]).is_none());
    }

    #[test]
    fn unnamed_location_gets_a_weight_name() {
        let inst = instance(&fixture(), &[(*b"wght", 650.0)]).unwrap();
        assert_eq!(inst.postscript_name, "KantorVarTest-Bold");
        let inst = instance(&fixture(), &[(*b"wght", 900.0)]).unwrap();
        // Clamped to the axis maximum (800, the named ExtraBold).
        assert_eq!(inst.weight, 800.0);
        assert_eq!(inst.postscript_name, "KantorVarTest-ExtraBold");
    }
}
