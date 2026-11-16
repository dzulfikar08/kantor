//! Vertical glyph forms: the substitutions a font's `vrt2` / `vert`
//! OpenType features make for East Asian vertical text (brackets turned
//! upright, the long vowel mark ー as a vertical bar, small kana moved to
//! the top right...). Text is shaped horizontally (parley), so the
//! substitution is applied to the shaped glyph ids here; a substituted
//! glyph is drawn upright, the font having drawn it for vertical text.
//! Fonts without the features keep today's turned glyphs.

use std::cell::RefCell;
use std::collections::HashMap;

use docs_layout::parley::FontData;
use skrifa::raw::tables::gsub::{ExtensionSubtable, SingleSubst, SubstitutionLookup};
use skrifa::raw::types::Tag;
use skrifa::raw::{FontRef, TableProvider};

type Map = HashMap<u16, u16>;

thread_local! {
    /// Each font's vertical substitutions, by blob id and index.
    static MAPS: RefCell<HashMap<(u64, u32), std::rc::Rc<Map>>> = RefCell::new(HashMap::new());
}

fn single(map: &mut Map, st: &SingleSubst) {
    match st {
        SingleSubst::Format1(t) => {
            let Ok(cov) = t.coverage() else { return };
            let d = t.delta_glyph_id() as i32;
            for g in cov.iter() {
                let g = g.to_u16();
                map.insert(g, (g as i32 + d).rem_euclid(65536) as u16);
            }
        }
        SingleSubst::Format2(t) => {
            let Ok(cov) = t.coverage() else { return };
            let subs = t.substitute_glyph_ids();
            for (i, g) in cov.iter().enumerate() {
                if let Some(s) = subs.get(i) {
                    map.insert(g.to_u16(), s.get().to_u16());
                }
            }
        }
    }
}

/// The font's vertical substitutions (`vert`, then `vrt2` over it).
fn read(data: &[u8], index: u32) -> Map {
    let mut map = Map::new();
    let Ok(font) = FontRef::from_index(data, index) else {
        return map;
    };
    let Ok(gsub) = font.gsub() else { return map };
    let (Ok(features), Ok(lookups)) = (gsub.feature_list(), gsub.lookup_list()) else {
        return map;
    };
    for tag in [Tag::new(b"vert"), Tag::new(b"vrt2")] {
        for rec in features.feature_records() {
            if rec.feature_tag() != tag {
                continue;
            }
            let Ok(feature) = rec.feature(features.offset_data()) else {
                continue;
            };
            for idx in feature.lookup_list_indices() {
                let Ok(lookup) = lookups.lookups().get(idx.get() as usize) else {
                    continue;
                };
                match lookup {
                    SubstitutionLookup::Single(l) => {
                        for st in l.subtables().iter().flatten() {
                            single(&mut map, &st);
                        }
                    }
                    SubstitutionLookup::Extension(l) => {
                        for ext in l.subtables().iter().flatten() {
                            if let ExtensionSubtable::Single(e) = ext
                                && let Ok(st) = e.extension()
                            {
                                single(&mut map, &st);
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    map
}

/// The vertical form of glyph `id` of `font`, if the font has one.
pub fn vertical_glyph(font: &FontData, id: u32) -> Option<u32> {
    let id16 = u16::try_from(id).ok()?;
    let key = (font.data.id(), font.index);
    let map = MAPS.with(|m| {
        let mut m = m.borrow_mut();
        if m.len() > 64 {
            m.clear();
        }
        m.entry(key)
            .or_insert_with(|| std::rc::Rc::new(read(font.data.data(), font.index)))
            .clone()
    });
    map.get(&id16).map(|g| *g as u32).filter(|g| *g != id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use skrifa::MetadataProvider;

    /// Noto Sans CJK has vertical forms of ー and （ (only checked where
    /// the font is installed: CI runners have no CJK fonts).
    #[test]
    fn cjk_fonts_give_vertical_forms() {
        let Ok(data) = std::fs::read("/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc") else {
            return;
        };
        let font = FontRef::from_index(&data, 0).unwrap();
        let cmap = font.charmap();
        let map = read(&data, 0);
        for c in ['ー', '（', '「', '、'] {
            let g = cmap.map(c).unwrap().to_u32() as u16;
            assert!(map.get(&g).is_some_and(|v| *v != g), "{c}");
        }
        // Their presentation forms are the vertical glyphs.
        for (c, v) in [('「', '﹁'), ('（', '︵')] {
            let g = cmap.map(c).unwrap().to_u32() as u16;
            let w = cmap.map(v).unwrap().to_u32() as u16;
            assert_eq!(map.get(&g), Some(&w), "{c}");
        }
        let a = cmap.map('A').unwrap().to_u32() as u16;
        assert!(!map.contains_key(&a));
    }
}
