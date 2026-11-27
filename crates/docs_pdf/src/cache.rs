//! Paragraph layouts kept between layouts of a document being edited.
//!
//! Layouts are keyed by a hash of everything that affects shaping and line
//! breaking, so an edit reshapes only the paragraphs it touched. Each pass
//! keeps only the layouts it used.

use std::collections::HashMap;
use std::hash::{Hash as _, Hasher as _};
use std::sync::Arc;

use docs_engine::Document;
use docs_layout::{Engine, ParaLayout};

use crate::Layout;

/// Cache key: everything that affects shaping and line breaking.
pub fn layout_key(p: &docs_layout::Paragraph, width: f32) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    p.text.hash(&mut h);
    p.family.hash(&mut h);
    p.size_pt.to_bits().hash(&mut h);
    format!("{:?}", p.line_height).hash(&mut h);
    (p.align as u8).hash(&mut h);
    width.to_bits().hash(&mut h);
    for v in [p.indent_left_pt, p.indent_right_pt, p.first_line_pt] {
        v.to_bits().hash(&mut h);
    }
    format!("{:?}{:?}{:?}{:?}", p.marker, p.objects, p.tabs, p.hidden).hash(&mut h);
    p.default_tab.to_bits().hash(&mut h);
    p.decimal.hash(&mut h);
    for s in &p.spans {
        s.range.hash(&mut h);
        (s.bold, s.italic, s.underline).hash(&mut h);
        s.size_pt.map(f32::to_bits).hash(&mut h);
        s.color.hash(&mut h);
        s.family.hash(&mut h);
    }
    h.finish()
}

/// Paragraph layouts of the last pass.
#[derive(Default)]
pub struct LayoutCache {
    paras: HashMap<u64, Arc<ParaLayout>>,
}

impl LayoutCache {
    /// Start a pass: lay out with [`Pass::lay_out`] (as often as needed),
    /// then [`Pass::finish`] keeps what it used.
    pub fn pass<'a>(&'a mut self, engine: &'a mut Engine) -> Pass<'a> {
        let fresh = HashMap::with_capacity(self.paras.len());
        Pass {
            cache: self,
            engine,
            fresh,
        }
    }

    /// Lay `doc` out once, reusing and then replacing the cache.
    pub fn lay_out(&mut self, doc: &Document, engine: &mut Engine) -> Layout {
        let mut pass = self.pass(engine);
        let laid = pass.lay_out(doc);
        pass.finish();
        laid
    }
}

/// One pass over a document (see [`LayoutCache::pass`]).
pub struct Pass<'a> {
    cache: &'a mut LayoutCache,
    engine: &'a mut Engine,
    fresh: HashMap<u64, Arc<ParaLayout>>,
}

impl Pass<'_> {
    pub fn lay_out(&mut self, doc: &Document) -> Layout {
        let (old, fresh, engine) = (&self.cache.paras, &mut self.fresh, &mut *self.engine);
        crate::lay_out_with(doc, &mut |p, width| {
            let key = layout_key(p, width);
            let layout = match old.get(&key).or_else(|| fresh.get(&key)) {
                Some(l) => l.clone(),
                None => Arc::new(engine.layout_paragraph(p, width)),
            };
            fresh.insert(key, layout.clone());
            layout
        })
    }

    pub fn finish(self) {
        self.cache.paras = self.fresh;
    }
}
