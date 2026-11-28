//! Kantor's equations: one model ([`ast`]) for Word's Office Math (OMML),
//! LaTeX, the linear format and Typst, and a layout engine for OpenType
//! MATH fonts ([`layout`]). No GPUI: Docs, Slides and the PDF exports draw
//! the positioned glyphs and rules it produces. See ADR 0008.

pub mod alnum;
pub mod ast;
pub mod latex;
pub mod layout;
pub mod linear;
pub mod omml;
pub mod symbols;
pub mod typst;

pub use ast::*;
pub use layout::{BUNDLED_FONT, Glyph, Layout, Options, layout};

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex};

/// Input that is LaTeX rather than the linear format: it has commands or
/// braces.
pub fn looks_like_latex(src: &str) -> bool {
    src.contains('\\') || src.contains('{') || src.contains('$')
}

/// What someone typed in an equation box: LaTeX (`\frac{a}{b}`) or
/// Word's linear format (`a/b`, `x^2`), told apart by
/// [`looks_like_latex`].
pub fn parse_input(src: &str, display: bool) -> Result<Equation, latex::Error> {
    if looks_like_latex(src) {
        latex::parse(src, display)
    } else {
        Ok(linear::parse(src, display))
    }
}

/// LaTeX for an equation stored as OMML ("copy as LaTeX", the editor).
pub fn omml_to_latex(xml: &str) -> Option<String> {
    omml::parse(xml).ok().map(|e| latex::write(&e))
}

/// The linear form of an equation stored as OMML (placeholder text,
/// search, shapes that don't draw math).
pub fn omml_to_linear(xml: &str) -> Option<String> {
    omml::parse(xml).ok().map(|e| linear::write(&e))
}

static CACHE: Mutex<Option<HashMap<u64, Arc<Layout>>>> = Mutex::new(None);
const CACHE_MAX: usize = 2048;

/// The layout of an equation stored as OMML at a font size (px), with
/// the bundled math font; cached by XML, size and width.
pub fn layout_omml(xml: &str, size: f32, width: Option<f32>) -> Option<Arc<Layout>> {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    xml.hash(&mut h);
    size.to_bits().hash(&mut h);
    width.map(f32::to_bits).hash(&mut h);
    let key = h.finish();
    if let Ok(c) = CACHE.lock()
        && let Some(l) = c.as_ref().and_then(|m| m.get(&key))
    {
        return Some(l.clone());
    }
    let eq = omml::parse(xml).ok()?;
    let l = Arc::new(layout(
        &eq,
        &Options {
            size,
            width,
            ..Options::default()
        },
    ));
    if let Ok(mut c) = CACHE.lock() {
        let m = c.get_or_insert_with(HashMap::new);
        if m.len() >= CACHE_MAX {
            m.clear();
        }
        m.insert(key, l.clone());
    }
    Some(l)
}
