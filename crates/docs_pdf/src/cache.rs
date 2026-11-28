//! Layouts kept between layouts of a document being edited.
//!
//! Paragraph layouts are keyed by a hash of everything that affects shaping
//! and line breaking, so an edit reshapes only the paragraphs it touched.
//! Each full pass keeps only the layouts it used.
//!
//! [`LayoutCache::relayout`] also keeps the last result's body: when the
//! only change since is one paragraph's text (typing, deleting, undoing
//! that) and its lines keep their heights, nothing else on any page moves,
//! so the new paragraph layout just replaces the old one in the last
//! result (no rebuild, no pagination, no headers and footers). Anything
//! else lays the whole document out again.

use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::{Hash as _, Hasher as _};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use docs_engine::{Document, Object};
use docs_layout::parley::FontContext;
use docs_layout::{COLUMN_BREAK, Engine, PAGE_BREAK, ParaLayout};

use crate::Layout;
use crate::build::{self, Marks};
use crate::pages::Body;

/// Feeds what's written to a hasher (hashing a `Debug` form without
/// making a string of it: every paragraph is keyed on every layout).
struct HashWrite<'a, H: std::hash::Hasher>(&'a mut H);

impl<H: std::hash::Hasher> std::fmt::Write for HashWrite<'_, H> {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        self.0.write(s.as_bytes());
        Ok(())
    }
}

/// Cache key: everything that affects shaping and line breaking.
pub fn layout_key(p: &docs_layout::Paragraph, width: f32) -> u64 {
    use std::fmt::Write as _;
    let mut h = std::collections::hash_map::DefaultHasher::new();
    p.text.hash(&mut h);
    p.family.hash(&mut h);
    p.size_pt.to_bits().hash(&mut h);
    let _ = write!(HashWrite(&mut h), "{:?}", p.line_height);
    (p.align as u8).hash(&mut h);
    width.to_bits().hash(&mut h);
    for v in [p.indent_left_pt, p.indent_right_pt, p.first_line_pt] {
        v.to_bits().hash(&mut h);
    }
    let _ = write!(
        HashWrite(&mut h),
        "{:?}{:?}{:?}{:?}",
        p.marker,
        p.objects,
        p.tabs,
        p.hidden
    );
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

/// What the last full layout knew about the body.
struct Last {
    /// The document's revision it (or a quick update since) laid out,
    /// which document that is, and its styles and lists then.
    revision: u64,
    instance: u64,
    styles: docs_engine::StyleSheet,
    numbering: docs_engine::numbering::Numbering,
    /// The document's paragraph storage (a stand-in for the document's
    /// identity: a one-paragraph edit doesn't move it).
    paras: usize,
    len: usize,
    /// The body as paginated: each paragraph's column width and layout
    /// before pagination, the layout paragraphs, blocks and sections.
    body: Body,
}

/// How the last [`LayoutCache::relayout`] went (for `KANTOR_PERF`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Outcome {
    #[default]
    Full,
    /// Only the start of the document ([`LayoutCache::relayout_prefix`]).
    Prefix,
    /// Paragraph `para`'s layout replaced.
    Quick { para: usize },
}

/// Paragraph layouts of the last pass, and the last result's body.
#[derive(Default)]
pub struct LayoutCache {
    paras: HashMap<u64, Arc<ParaLayout>>,
    last: Option<Last>,
    /// How the last relayout went.
    pub outcome: Outcome,
    /// After a full relayout: from which page on the pages were the last
    /// layout's, and which page of it that was.
    pub reused: Option<(usize, usize)>,
    /// After [`Self::relayout_prefix`]: about how many pages the whole
    /// document has.
    pub estimate: Option<usize>,
}

impl LayoutCache {
    /// Start a pass: lay out with [`Pass::lay_out`] (as often as needed),
    /// then [`Pass::finish`] keeps what it used. The next
    /// [`Self::relayout`] lays the whole document out.
    pub fn pass<'a>(&'a mut self, engine: &'a mut Engine) -> Pass<'a> {
        let fresh = HashMap::with_capacity(self.paras.len());
        self.last = None;
        self.outcome = Outcome::Full;
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

    /// Lay `doc` out again after an edit. `prev` is the last result of
    /// this cache for `doc` (if the caller still has it to give back): a
    /// one-paragraph text edit that keeps its lines' heights updates it in
    /// place.
    pub fn relayout(
        &mut self,
        doc: &Document,
        engine: &mut Engine,
        prev: Option<Layout>,
    ) -> Layout {
        let mut prev = prev;
        if let Some(prev) = prev.as_mut() {
            let _t = docs_layout::perf::span("docs quick relayout");
            let strikes = !prev.strikes.is_empty();
            if let Some(para) = self.quick(doc, engine, prev) {
                self.outcome = Outcome::Quick { para };
                let last = self
                    .last
                    .as_mut()
                    .expect("quick updates need the last layout");
                if strikes {
                    // The struck-through paragraphs are the body's layout
                    // paragraphs (one shared list): updated there.
                    prev.strikes = last.body.lps.clone();
                }
                return std::mem::replace(prev, empty_layout());
            }
        }
        self.outcome = Outcome::Full;
        self.estimate = None;
        // The last layout's pages that come out the same are kept, and
        // after a known edit what it didn't change is taken over.
        let mut last = self.last.take();
        if let Some(p) = prev.as_mut() {
            // (The struck-through paragraphs are the body's: let go.)
            p.strikes = Arc::default();
        }
        let splice = last.as_ref().and_then(|l| known_splice(doc, l));
        let reuse = prev
            .as_ref()
            .zip(last.as_mut())
            .map(|(laid, l)| crate::pages::Prev {
                laid,
                body: &mut l.body,
                splice,
            });
        let (laid, body) = self.full(doc, engine, None, reuse);
        self.reused = laid.paged.reused();
        drop(prev);
        drop(last);
        self.last = (body.shaped.len() == doc.len()).then(|| Last {
            revision: doc.revision(),
            instance: doc.instance(),
            styles: doc.styles.clone(),
            numbering: doc.numbering.clone(),
            paras: doc.paragraphs().as_ptr() as usize,
            len: doc.len(),
            body,
        });
        laid
    }

    /// Lay out only the start of `doc`, about its first `limit`
    /// paragraphs (a long document's first pages, shown while
    /// [`lay_out_parallel`] lays out the rest). The pages after aren't
    /// there yet, and the next [`Self::relayout`] lays out everything.
    pub fn relayout_prefix(&mut self, doc: &Document, engine: &mut Engine, limit: usize) -> Layout {
        let (laid, _) = self.full(doc, engine, Some(limit), None);
        self.last = None;
        self.outcome = Outcome::Prefix;
        self.estimate = Some(crate::pages::estimate_pages(doc, limit, &laid.paged));
        laid
    }

    /// Take a [`lay_out_parallel`] result for `doc`: its layout when
    /// `doc` hasn't changed since (later edits go on from it), else `None`
    /// (its paragraph layouts are kept: the next relayout reuses them).
    pub fn install(&mut self, doc: &Document, bg: Background) -> Option<Layout> {
        self.estimate = None;
        if doc.revision() != bg.revision || doc.len() != bg.body.shaped.len() {
            self.paras.extend(bg.paras);
            self.last = None;
            return None;
        }
        self.paras = bg.paras;
        self.last = Some(Last {
            revision: bg.revision,
            instance: doc.instance(),
            styles: doc.styles.clone(),
            numbering: doc.numbering.clone(),
            paras: doc.paragraphs().as_ptr() as usize,
            len: doc.len(),
            body: bg.body,
        });
        self.outcome = Outcome::Full;
        Some(bg.laid)
    }

    /// A full layout with the cache (only the start with `limit`); the
    /// cache keeps the paragraph layouts it used.
    fn full(
        &mut self,
        doc: &Document,
        engine: &mut Engine,
        limit: Option<usize>,
        prev: Option<crate::pages::Prev>,
    ) -> (Layout, Body) {
        let edit = prev.as_ref().is_some_and(|p| p.splice.is_some());
        let fresh = RefCell::new(HashMap::with_capacity(if edit {
            64
        } else {
            self.paras.len()
        }));
        let laid = lay_out_shared(doc, &self.paras, &fresh, engine, limit, prev);
        let fresh = fresh.into_inner();
        // After an edit only what changed was looked up: keep the rest
        // (until there's much more than the document needs).
        if edit && self.paras.len() < 2 * doc.len() + 4096 {
            self.paras.extend(fresh);
        } else {
            self.paras = fresh;
        }
        laid
    }

    /// The quick update (see the module docs), or `None` when it doesn't
    /// apply.
    fn quick(&mut self, doc: &Document, engine: &mut Engine, prev: &mut Layout) -> Option<usize> {
        let last = self.last.as_mut()?;
        let sp = doc.last_splice()?;
        if sp.revision != last.revision + 1
            || sp.removed != 1
            || sp.inserted != 1
            || sp.side
            || sp.old.len() != 1
            || doc.len() != last.len
            || doc.paragraphs().as_ptr() as usize != last.paras
            || doc.field_codes().is_some()
            || doc.merge_view().is_some()
        {
            return None;
        }
        let i = sp.start;
        let shaped = &last.body.shaped;
        if i >= shaped.len()
            || prev.paged.layouts.len() != shaped.len()
            || !Arc::ptr_eq(&prev.paged.layouts[i], &shaped[i])
        {
            return None;
        }
        let (old_p, new_p) = (&sp.old[0], doc.paragraph(i));
        if old_p.props != new_p.props
            || old_p.cell != new_p.cell
            || !plain(old_p)
            || !plain(new_p)
            || last
                .body
                .style_refs
                .iter()
                .any(|r| r.shows(doc, old_p) || r.shows(doc, new_p))
        {
            return None;
        }
        let width = last.body.widths[i];
        let marker = shaped[i].marker.as_ref().map(|m| m.2.as_str());
        let marks = Marks::default();
        let old_lp = build::layout_paragraph(doc, old_p, marker, width, &marks);
        let mut new_lp = build::layout_paragraph(doc, new_p, marker, width, &marks);
        if !new_lp.floats.is_empty()
            || !old_lp.floats.is_empty()
            || structure(&old_lp) != structure(&new_lp)
        {
            return None;
        }
        let key = layout_key(&new_lp, width);
        let shaped = match self.paras.get(&key) {
            Some(l) => l.clone(),
            None => Arc::new(engine.layout_paragraph(&new_lp, width)),
        };
        if !same_lines(&last.body.shaped[i], &shaped) {
            return None;
        }
        // Struck-through text (tracked deletions) is drawn from the layout
        // paragraphs, kept when some text is struck through: the body's
        // list (the caller shares it again).
        if !prev.strikes.is_empty() {
            if last.body.lps.len() <= i {
                return None;
            }
            prev.strikes = Arc::default();
            let lps = Arc::make_mut(&mut last.body.lps);
            // Spacing comes from the neighbours (contextual spacing).
            new_lp.space_before_pt = lps[i].space_before_pt;
            new_lp.space_after_pt = lps[i].space_after_pt;
            lps[i] = new_lp;
        } else if !new_lp.strikes.is_empty() {
            return None;
        }
        prev.paged.layouts[i] = shaped.clone();
        last.body.shaped[i] = shaped;
        last.revision = sp.revision;
        Some(i)
    }
}

/// A full layout made away from the view (see [`lay_out_parallel`]), for
/// [`LayoutCache::install`].
pub struct Background {
    laid: Layout,
    body: Body,
    paras: HashMap<u64, Arc<ParaLayout>>,
    revision: u64,
}

impl Background {
    pub fn pages(&self) -> usize {
        self.laid.paged.pages.len()
    }
}

/// Lay `doc` out (all of it) on other threads than the caller's: give
/// it a clone of the view engine's fonts with a shared source cache
/// (`SourceCache::make_shared`), so every thread uses the font files the
/// view already loaded. For a long document opened in the view: run it in
/// the background, show [`LayoutCache::relayout_prefix`] meanwhile.
pub fn lay_out_parallel(doc: &Document, fonts: &FontContext) -> Background {
    let mut engine = Engine::with_fonts(fonts.clone());
    let fresh = RefCell::new(HashMap::new());
    let (laid, body) = lay_out_shared(doc, &HashMap::new(), &fresh, &mut engine, None, None);
    Background {
        laid,
        body,
        paras: fresh.into_inner(),
        revision: doc.revision(),
    }
}

type Shaped = HashMap<u64, Arc<ParaLayout>>;

/// A layout of `doc` taking paragraph layouts from `old` (and `fresh`),
/// keeping every one it uses in `fresh`. The body's paragraphs are keyed
/// and those not laid out yet shaped on several threads.
fn lay_out_shared(
    doc: &Document,
    old: &Shaped,
    fresh: &RefCell<Shaped>,
    engine: &mut Engine,
    limit: Option<usize>,
    prev: Option<crate::pages::Prev>,
) -> (Layout, Body) {
    let engine = RefCell::new(engine);
    let mut one = |p: &docs_layout::Paragraph, width: f32| {
        let key = layout_key(p, width);
        let hit = old
            .get(&key)
            .cloned()
            .or_else(|| fresh.borrow().get(&key).cloned());
        if let Some(l) = hit {
            fresh.borrow_mut().insert(key, l.clone());
            return l;
        }
        let l = Arc::new(engine.borrow_mut().layout_paragraph(p, width));
        fresh.borrow_mut().insert(key, l.clone());
        l
    };
    let mut batch = |lps: &[docs_layout::Paragraph], widths: &[f32]| {
        shape_body(lps, widths, old, fresh, &mut engine.borrow_mut())
    };
    crate::pages::lay_out_body(
        doc,
        &mut one,
        limit,
        Some(&mut batch as &mut crate::pages::BatchFn<'_>),
        prev,
    )
}

/// The one edit made to `doc` since `last` was laid out, when that's
/// known: `.1` paragraphs from `.0` on became `.2` (styles, lists and
/// everything else as they were).
fn known_splice(doc: &Document, last: &Last) -> Option<(usize, usize, usize)> {
    let sp = doc.last_splice()?;
    (doc.instance() == last.instance
        && sp.revision == last.revision + 1
        && !sp.side
        && sp.removed + sp.inserted > 0
        && doc.styles == last.styles
        && doc.numbering == last.numbering)
        .then_some((sp.start, sp.removed, sp.inserted))
}

/// A layout of nothing (what's left where one was taken out).
fn empty_layout() -> Layout {
    Layout {
        paged: docs_layout::PagedDocument::paginate(&[], Vec::new(), docs_layout::PageSetup::a4()),
        furniture: Vec::new(),
        floats: Vec::new(),
        strikes: Arc::default(),
    }
}

/// The layouts of body paragraphs `lps` at `widths`: keys hashed on
/// several threads, layouts from `old` or `fresh`, the missing ones shaped
/// on several threads (on `engine` when there are only a few).
fn shape_body(
    lps: &[docs_layout::Paragraph],
    widths: &[f32],
    old: &Shaped,
    fresh: &RefCell<Shaped>,
    engine: &mut Engine,
) -> Vec<Arc<ParaLayout>> {
    let width = |k: usize| widths.get(k).copied().unwrap_or(100.0);
    let keys = crate::par::map(lps.len(), &|k| layout_key(&lps[k], width(k)));
    let mut missing = Vec::new();
    {
        let have = fresh.borrow();
        let mut seen = std::collections::HashSet::new();
        for (k, key) in keys.iter().enumerate() {
            if !old.contains_key(key) && !have.contains_key(key) && seen.insert(*key) {
                missing.push(k);
            }
        }
    }
    let threads = crate::par::threads();
    if missing.len() >= 64 && threads > 1 {
        let _t = docs_layout::perf::span("docs shape (threads)");
        engine.fonts.source_cache.make_shared();
        let fonts = engine.fonts.clone();
        let shaped = shape_parallel(&fonts, lps, &missing, &width, threads);
        fresh
            .borrow_mut()
            .extend(missing.iter().map(|&k| keys[k]).zip(shaped));
    } else {
        let mut have = fresh.borrow_mut();
        for &k in &missing {
            have.insert(
                keys[k],
                Arc::new(engine.layout_paragraph(&lps[k], width(k))),
            );
        }
    }
    let mut have = fresh.borrow_mut();
    keys.iter()
        .zip(lps)
        .enumerate()
        .map(|(k, (key, p))| {
            let l = match old.get(key).or_else(|| have.get(key)) {
                Some(l) => l.clone(),
                // Shaping failed on its thread: here.
                None => Arc::new(engine.layout_paragraph(p, width(k))),
            };
            have.insert(*key, l.clone());
            l
        })
        .collect()
}

/// Paragraphs `which` of `lps` shaped on `threads` threads (each with a
/// clone of `fonts`), in the order of `which`.
fn shape_parallel(
    fonts: &FontContext,
    lps: &[docs_layout::Paragraph],
    which: &[usize],
    width: &(dyn Fn(usize) -> f32 + Sync),
    threads: usize,
) -> Vec<Arc<ParaLayout>> {
    const BATCH: usize = 16;
    let next = AtomicUsize::new(0);
    let mut out: Vec<(usize, Arc<ParaLayout>)> = std::thread::scope(|s| {
        let workers: Vec<_> = (0..threads.max(1))
            .map(|_| {
                let fonts = fonts.clone();
                let next = &next;
                s.spawn(move || {
                    let mut engine = Engine::with_fonts(fonts);
                    let mut out = Vec::new();
                    loop {
                        let start = next.fetch_add(BATCH, Ordering::Relaxed);
                        if start >= which.len() {
                            break;
                        }
                        for (j, &k) in which.iter().enumerate().skip(start).take(BATCH) {
                            out.push((j, Arc::new(engine.layout_paragraph(&lps[k], width(k)))));
                        }
                    }
                    out
                })
            })
            .collect();
        workers
            .into_iter()
            .flat_map(|w| w.join().unwrap_or_default())
            .collect()
    });
    out.sort_by_key(|(j, _)| *j);
    // A worker that panicked left its paragraphs out: none for them (the
    // caller shapes them itself).
    if out.len() != which.len() {
        return Vec::new();
    }
    out.into_iter().map(|(_, l)| l).collect()
}

/// A paragraph whose text changes can't move anything but its own lines:
/// no page or column breaks, footnote references or floating objects.
fn plain(p: &docs_engine::Paragraph) -> bool {
    if p.text.contains([PAGE_BREAK, COLUMN_BREAK]) {
        return false;
    }
    p.objects().all(|(_, o)| match o {
        Object::Float(_) => false,
        Object::Opaque(op) => docs_engine::notes::reference_in(&op.xml).is_none(),
        _ => true,
    })
}

/// Everything about a layout paragraph but its text and where its runs
/// and objects are: what places its lines on the page.
fn structure(p: &docs_layout::Paragraph) -> String {
    let objects: Vec<_> = p
        .objects
        .iter()
        .map(|o| (o.kind, o.width.to_bits(), o.height.to_bits(), &o.label))
        .collect();
    format!(
        "{:?}",
        (
            (p.align, &p.family, p.size_pt, p.line_spacing, p.line_height),
            (p.space_before_pt, p.space_after_pt),
            (p.indent_left_pt, p.indent_right_pt, p.first_line_pt),
            (&p.marker, objects, p.page_break_before, p.keep_next),
            (&p.locale, &p.tabs, p.default_tab, p.decimal, p.no_wrap),
            (&p.notes, &p.note_splits, &p.note_widths),
        )
    )
}

/// The same lines, as tall, at the same place.
fn same_lines(a: &ParaLayout, b: &ParaLayout) -> bool {
    a.layout.len() == b.layout.len()
        && a.dx == b.dx
        && a.shift == b.shift
        && a.width == b.width
        && a.layout
            .lines()
            .zip(b.layout.lines())
            .all(|(x, y)| x.metrics().line_height.to_bits() == y.metrics().line_height.to_bits())
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

/// The first difference between two layouts of one document as drawn:
/// pages, where every line sits, every glyph, struck-through text, objects,
/// headers and footers, floating objects. `None`: they draw the same.
/// (Tests and `kantor docs bench --verify` check quick updates with it.)
pub fn difference(a: &Layout, b: &Layout) -> Option<String> {
    let (pa, pb) = (&a.paged, &b.paged);
    if pa.pages.len() != pb.pages.len() {
        return Some(format!("{} pages vs {}", pa.pages.len(), pb.pages.len()));
    }
    for page in 0..pa.pages.len() {
        if pa.pages[page] != pb.pages[page] {
            return Some(format!("page {} placed differently", page + 1));
        }
        let runs = |p: &docs_layout::PagedDocument| -> Vec<String> {
            p.glyph_runs(page)
                .iter()
                .map(|r| {
                    let glyphs: Vec<_> = r
                        .glyphs
                        .iter()
                        .map(|g| (g.id, g.x.to_bits(), g.y.to_bits(), g.cluster.clone()))
                        .collect();
                    format!(
                        "{} {:?} {:?} {} {:?} {:?}",
                        r.para, r.text_range, r.source, r.font_size, r.color, glyphs
                    )
                })
                .collect()
        };
        if runs(pa) != runs(pb) {
            return Some(format!("page {} glyphs differ", page + 1));
        }
        if a.strike_rules(page) != b.strike_rules(page) {
            return Some(format!("page {} strikes differ", page + 1));
        }
        if pa.objects(page) != pb.objects(page) {
            return Some(format!("page {} objects differ", page + 1));
        }
        let furniture = |l: &Layout| -> Vec<String> {
            l.furniture[page]
                .iter()
                .map(|p| {
                    let texts: Vec<&str> = p.layout.paras.iter().map(|q| q.text.as_str()).collect();
                    format!("{:?} {} {} {:?} {:?}", p.kind, p.x, p.y, p.note, texts)
                })
                .collect()
        };
        if furniture(a) != furniture(b) {
            return Some(format!("page {} header, footer or notes differ", page + 1));
        }
        let floats = |l: &Layout| -> Vec<String> {
            l.floats[page]
                .iter()
                .map(|f| format!("{} {} {:?} {:?}", f.para, f.index, f.story, f.rect))
                .collect()
        };
        if floats(a) != floats(b) {
            return Some(format!("page {} floating objects differ", page + 1));
        }
    }
    None
}
