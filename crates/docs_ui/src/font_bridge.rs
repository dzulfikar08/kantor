//! fontique face → GPUI `FontId` (ADR 0001, "The crux").
//!
//! parley's glyph ids only mean something for the exact font file it shaped
//! with, so each face is mapped to the GPUI `FontId` of the *same file* once
//! and cached:
//!
//! 1. Read the face's family name, weight and style with skrifa and resolve
//!    them with GPUI. Accept the result only if GPUI reports the same family
//!    and its advance widths match the face's `hmtx` for sample characters.
//! 2. Faces GPUI won't take as a primary font (no Latin `m`, e.g. Arabic) are
//!    reached through its fallback path: shape the run's text with the
//!    family as a fallback and take the run's `FontId` (undocumented
//!    behaviour; ADR risk 2).
//! 3. Otherwise register the face's bytes with GPUI and resolve again.
//!
//! A face that still doesn't verify maps to `None` and its glyphs aren't
//! painted (logged once) rather than painted with the wrong glyph ids.

use std::borrow::Cow;
use std::collections::HashMap;

use docs_layout::parley::FontData;
use gpui_kit::*;
use skrifa::raw::TableProvider as _;
use skrifa::{MetadataProvider as _, string::StringId};

#[derive(Default)]
pub struct FontBridge {
    faces: HashMap<(u64, u32), Option<FontId>>,
}

/// What GPUI needs to know about a parley face.
struct Face {
    family: String,
    weight: f32,
    italic: bool,
    upem: f32,
    /// `(char, advance in font units)` for verification.
    samples: Vec<(char, f32)>,
}

fn describe(font: &FontData) -> Option<Face> {
    let f = skrifa::FontRef::from_index(font.data.data(), font.index).ok()?;
    let family = f
        .localized_strings(StringId::TYPOGRAPHIC_FAMILY_NAME)
        .english_or_first()
        .or_else(|| {
            f.localized_strings(StringId::FAMILY_NAME)
                .english_or_first()
        })?
        .to_string();
    let attrs = f.attributes();
    let upem = f.head().ok()?.units_per_em() as f32;
    let cmap = f.charmap();
    let hmtx = f.hmtx().ok()?;
    let mut samples = Vec::new();
    for ch in "mHaenos0.,ABCabc".chars() {
        if let Some(gid) = cmap.map(ch)
            && let Some(adv) = hmtx.advance(gid)
        {
            samples.push((ch, adv as f32));
        }
    }
    if samples.len() < 4 {
        // Script-only faces: sample a spread of their own code points.
        for (cp, gid) in cmap.mappings().step_by(97).take(24) {
            if let (Some(ch), Some(adv)) = (char::from_u32(cp), hmtx.advance(gid)) {
                samples.push((ch, adv as f32));
            }
        }
    }
    Some(Face {
        family,
        weight: attrs.weight.value(),
        italic: !matches!(attrs.style, skrifa::attribute::Style::Normal),
        upem,
        samples,
    })
}

impl FontBridge {
    /// The GPUI font for a parley face. `sample` is text the face shaped
    /// (used to reach script-only faces through GPUI's fallback path).
    pub fn font_id(
        &mut self,
        font: &FontData,
        sample: &str,
        ts: &WindowTextSystem,
    ) -> Option<FontId> {
        let key = (font.data.id(), font.index);
        if let Some(id) = self.faces.get(&key) {
            return *id;
        }
        let id = Self::resolve(font, sample, ts);
        if id.is_none() {
            log::warn!(
                "docs: no GPUI font for face #{} index {}; its glyphs are not painted",
                key.0,
                key.1
            );
        }
        self.faces.insert(key, id);
        id
    }

    fn resolve(font: &FontData, sample: &str, ts: &WindowTextSystem) -> Option<FontId> {
        let face = describe(font)?;
        let mut f = gpui_kit::font(face.family.clone());
        f.weight = FontWeight(face.weight);
        f.style = if face.italic {
            FontStyle::Italic
        } else {
            FontStyle::Normal
        };
        let same = |id: FontId| verify(&face, id, ts);

        let id = ts.resolve_font(&f);
        if same(id) {
            return Some(id);
        }
        // Script-only faces: GPUI's fallback path (ADR risk 2).
        if !sample.is_empty() {
            let mut primary = gpui_kit::font("Liberation Sans");
            primary.fallbacks = Some(FontFallbacks::from_fonts(vec![face.family.clone()]));
            let pid = ts.resolve_font(&primary);
            let run = TextRun {
                len: sample.len(),
                font: primary,
                color: Hsla::default(),
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            let line = ts.layout_line(sample, px(16.), &[run], None);
            if let Some(id) = line
                .runs
                .iter()
                .map(|r| r.font_id)
                .find(|id| *id != pid && same(*id))
            {
                return Some(id);
            }
        }
        // A face only fontique knows (or a different file of the same name):
        // give GPUI the exact bytes.
        let bytes = font.data.data().to_vec();
        if ts.add_fonts(vec![Cow::Owned(bytes)]).is_ok() {
            let id = ts.resolve_font(&f);
            if same(id) {
                return Some(id);
            }
        }
        None
    }
}

/// Same family name and the same advances as the face's own `hmtx`.
/// Faces GPUI loaded only as fallbacks have no `Font` record; for those the
/// advances decide alone, so at least a few distinct ones must match.
fn verify(face: &Face, id: FontId, ts: &TextSystem) -> bool {
    let family_ok = match ts.get_font_for_id(id) {
        Some(f) => f.family.as_ref().eq_ignore_ascii_case(&face.family),
        None => face.samples.iter().filter(|(_, adv)| *adv > 0.0).count() >= 3,
    };
    if !family_ok || face.samples.is_empty() {
        return false;
    }
    let size = px(face.upem);
    face.samples.iter().all(|(ch, adv)| {
        ts.advance(id, size, *ch)
            .is_ok_and(|a| (f32::from(a.width) - adv).abs() <= 0.5 + adv * 0.002)
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use docs_layout::{Engine, PageSetup, sample};

    use super::*;

    /// ADR 0001 risk 2: every face parley picks for the sample document
    /// (Latin, CJK, and the Arabic face GPUI only reaches as a fallback)
    /// maps to a verified GPUI font. Skipped where the sample fonts aren't
    /// installed.
    #[std::prelude::v1::test]
    fn sample_faces_map_to_gpui_fonts() {
        let platform = Arc::new(gpui_wgpu::CosmicTextSystem::new("Liberation Sans"));
        let ts = WindowTextSystem::new(Arc::new(TextSystem::new(platform)));
        let mut engine = Engine::new();
        let paras = sample::document(2);
        let doc = engine.layout_document(&paras, PageSetup::a4());
        let mut bridge = FontBridge::default();
        let (mut faces, mut mapped) = (0, 0);
        let mut seen = std::collections::HashSet::new();
        for run in doc.glyph_runs(0) {
            if !seen.insert((run.font.data.id(), run.font.index)) {
                continue;
            }
            faces += 1;
            let sample = &paras[run.para].text[run.text_range.clone()];
            let name = describe(&run.font).map(|f| f.family).unwrap_or_default();
            match bridge.font_id(&run.font, sample, &ts) {
                Some(_) => mapped += 1,
                None => eprintln!("unmapped face: {name}"),
            }
        }
        let has = |family: &str| {
            ts.all_font_names()
                .iter()
                .any(|n| n.eq_ignore_ascii_case(family))
        };
        if !(has("Carlito") && has("Noto Sans Arabic")) {
            eprintln!("sample fonts not installed; skipping strict check");
            return;
        }
        assert!(faces >= 3, "{faces} faces");
        assert_eq!(mapped, faces, "every face maps to the same GPUI font file");
    }
}
