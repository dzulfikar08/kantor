//! The pages of an edited PDF: a list of pages of *sources* (the file
//! opened, files inserted, pages pasted from other open PDFs, blank pages,
//! the result of a watermark…), each with an extra rotation. Reordering,
//! deleting, rotating, inserting and duplicating only edit the list, so
//! they are instant even in long files and pages already drawn stay drawn;
//! Save builds the PDF with `kantor_pdf::assemble`. Every change is one
//! undo step.

use std::collections::{BTreeSet, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

use kantor_pdf::annot::Annot;
use kantor_pdf::form::{FieldValue, Form};
use pdf_engine::{OutlineItem, PdfDoc, Turn};

/// A source's id, unique in the process (pages pasted between windows keep
/// theirs).
pub(crate) type SrcId = u64;

/// A page of a source: its id and page index.
pub(crate) type PageKey = (SrcId, usize);

pub(crate) fn next_id() -> SrcId {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// A PDF whose pages the document uses, held in memory.
pub(crate) struct Source {
    pub id: SrcId,
    /// The file name it came from (for messages).
    pub name: String,
    pub bytes: Arc<Vec<u8>>,
    /// The password that opened it (`""` for none).
    pub password: String,
    /// Each page's size as shown, points (its own rotation applied).
    pub sizes: Vec<(f32, f32)>,
    pub outline: Vec<OutlineItem>,
    pub title: Option<String>,
    /// Each page's map from PDF user space to page space.
    pub xf: Vec<[f64; 6]>,
    /// Its annotations and form, read once when first needed.
    pub marks: OnceLock<Option<Arc<SourceMarks>>>,
}

/// A source's annotations and form as `kantor_pdf` reads them.
pub(crate) struct SourceMarks {
    pub pdf: kantor_pdf::Pdf,
    /// Each page's annotations in the file.
    pub pages: Vec<Arc<Vec<Annot>>>,
    pub form: Option<Form>,
    /// Its digital signatures, if signed.
    pub sigs: Option<kantor_pdf::sig::Signatures>,
}

impl Source {
    /// Read `bytes` with hayro (on any thread): page sizes, bookmarks,
    /// title. Fails like [`PdfDoc::open`].
    pub fn read(
        name: String,
        bytes: Arc<Vec<u8>>,
        password: String,
    ) -> Result<Self, pdf_engine::OpenError> {
        let doc = PdfDoc::open(bytes.clone(), &password)?;
        Ok(Self {
            id: next_id(),
            name,
            xf: (0..doc.page_count()).map(|i| doc.user_to_page(i)).collect(),
            marks: OnceLock::new(),
            sizes: doc.page_sizes(),
            outline: doc.outline(),
            title: doc.title(),
            bytes,
            password,
        })
    }

    /// Its annotations and form (read with lopdf the first time: call off
    /// the UI thread for big files). `None` when lopdf can't read it.
    pub fn marks(&self) -> Option<&Arc<SourceMarks>> {
        self.marks
            .get_or_init(|| {
                let pdf = self.lopdf().ok()?;
                let n = pdf.page_count();
                let pages = (0..n)
                    .map(|p| Arc::new(kantor_pdf::annot::read_page(&pdf, p)))
                    .collect();
                let form = kantor_pdf::form::read_form(&pdf);
                let sigs = kantor_pdf::sig::signatures(&pdf);
                Some(Arc::new(SourceMarks {
                    pdf,
                    pages,
                    form,
                    sigs,
                }))
            })
            .as_ref()
    }

    /// Already read ([`Self::marks`] won't block).
    pub fn marks_ready(&self) -> Option<&Arc<SourceMarks>> {
        self.marks.get().and_then(Option::as_ref)
    }

    /// Page `page`'s point `(x, y)` of page space in PDF user space.
    pub fn to_user(&self, page: usize, x: f32, y: f32) -> [f32; 2] {
        let m = self
            .xf
            .get(page)
            .copied()
            .unwrap_or([1.0, 0.0, 0.0, -1.0, 0.0, 792.0]);
        let det = m[0] * m[3] - m[1] * m[2];
        if det.abs() < 1e-12 {
            return [x, y];
        }
        let (x, y) = (f64::from(x) - m[4], f64::from(y) - m[5]);
        [
            ((m[3] * x - m[2] * y) / det) as f32,
            ((-m[1] * x + m[0] * y) / det) as f32,
        ]
    }

    /// A point of user space on page `page` in page space.
    pub fn to_page(&self, page: usize, x: f32, y: f32) -> [f32; 2] {
        let m = self
            .xf
            .get(page)
            .copied()
            .unwrap_or([1.0, 0.0, 0.0, -1.0, 0.0, 792.0]);
        let (x, y) = (f64::from(x), f64::from(y));
        [
            (m[0] * x + m[2] * y + m[4]) as f32,
            (m[1] * x + m[3] * y + m[5]) as f32,
        ]
    }

    /// A user-space rectangle as a page-space one.
    pub fn rect_to_page(&self, page: usize, r: [f32; 4]) -> [f32; 4] {
        let a = self.to_page(page, r[0], r[1]);
        let b = self.to_page(page, r[2], r[3]);
        [
            a[0].min(b[0]),
            a[1].min(b[1]),
            a[0].max(b[0]),
            a[1].max(b[1]),
        ]
    }

    /// The source as a `kantor_pdf` document (decrypted).
    pub fn lopdf(&self) -> kantor_pdf::Result<kantor_pdf::Pdf> {
        match kantor_pdf::Pdf::from_bytes(&self.bytes, None) {
            Err(kantor_pdf::Error::Encrypted) => {
                kantor_pdf::Pdf::from_bytes(&self.bytes, Some(&self.password))
            }
            r => r,
        }
    }
}

/// One page of the document.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct PageRef {
    pub src: SrcId,
    pub page: usize,
    /// Quarter turns clockwise on top of the page's own rotation.
    pub rot: u8,
}

impl PageRef {
    pub fn key(self) -> PageKey {
        (self.src, self.page)
    }
}

/// What a change did, for Undo / Redo's wording.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Change {
    Move,
    Delete,
    Rotate,
    Insert,
    Duplicate,
    Paste,
    Watermark,
    PageNumbers,
    /// An annotation or form change: its name for Undo (an English
    /// catalog key).
    Markup(&'static str),
}

/// Annotation lists that replace a page's own, by source page.
pub(crate) type Marks = HashMap<PageKey, Arc<Vec<Annot>>>;
/// Form values set, by source and field name.
pub(crate) type Fields = HashMap<SrcId, Arc<HashMap<String, FieldValue>>>;

#[derive(Clone, Debug, PartialEq)]
struct State {
    base: SrcId,
    pages: Vec<PageRef>,
    rev: u64,
    marks: Arc<Marks>,
    fields: Arc<Fields>,
}

pub(crate) struct Model {
    sources: HashMap<SrcId, Arc<Source>>,
    cur: State,
    undo: Vec<(State, Change)>,
    redo: Vec<(State, Change)>,
    saved: u64,
    next_rev: u64,
    /// The first place of each source page (bookmarks, links).
    index: HashMap<PageKey, usize>,
}

impl Model {
    /// A document showing all of `source`.
    pub fn new(source: Arc<Source>) -> Self {
        let pages = all_pages(&source);
        let base = source.id;
        let mut m = Self {
            sources: HashMap::from([(base, source)]),
            cur: State {
                base,
                pages,
                rev: 0,
                marks: Arc::default(),
                fields: Arc::default(),
            },
            undo: Vec::new(),
            redo: Vec::new(),
            saved: 0,
            next_rev: 1,
            index: HashMap::new(),
        };
        m.reindex();
        m
    }

    fn reindex(&mut self) {
        self.index.clear();
        for (i, p) in self.cur.pages.iter().enumerate() {
            self.index.entry(p.key()).or_insert(i);
        }
    }

    pub fn pages(&self) -> &[PageRef] {
        &self.cur.pages
    }

    pub fn len(&self) -> usize {
        self.cur.pages.len()
    }

    pub fn page(&self, i: usize) -> Option<PageRef> {
        self.cur.pages.get(i).copied()
    }

    pub fn base(&self) -> &Arc<Source> {
        &self.sources[&self.cur.base]
    }

    pub fn source(&self, id: SrcId) -> Option<&Arc<Source>> {
        self.sources.get(&id)
    }

    pub fn add_source(&mut self, s: Arc<Source>) {
        self.sources.insert(s.id, s);
    }

    /// Page `i`'s size as shown (points, its rotation applied, not the
    /// view's).
    pub fn size(&self, i: usize) -> (f32, f32) {
        let Some(p) = self.page(i) else {
            return (612.0, 792.0);
        };
        let (w, h) = self.unturned_size(p);
        Turn(p.rot).size(w, h)
    }

    /// A page's size without its extra rotation (the space its text and
    /// links are in).
    pub fn unturned_size(&self, p: PageRef) -> (f32, f32) {
        self.sources
            .get(&p.src)
            .and_then(|s| s.sizes.get(p.page).copied())
            .unwrap_or((612.0, 792.0))
    }

    pub fn sizes(&self) -> Vec<(f32, f32)> {
        (0..self.len()).map(|i| self.size(i)).collect()
    }

    /// Where page `page` of source `src` is now (its first copy).
    pub fn index_of(&self, key: PageKey) -> Option<usize> {
        self.index.get(&key).copied()
    }

    pub fn revision(&self) -> u64 {
        self.cur.rev
    }

    pub fn is_dirty(&self) -> bool {
        self.cur.rev != self.saved
    }

    /// The document as it was at `rev` is on disk now.
    pub fn mark_saved(&mut self, rev: u64) {
        self.saved = rev;
    }

    #[cfg(test)]
    pub fn redo_change(&self) -> Option<Change> {
        self.redo.last().map(|u| u.1)
    }

    fn commit(&mut self, base: SrcId, pages: Vec<PageRef>, change: Change) {
        let marks = self.cur.marks.clone();
        let fields = self.cur.fields.clone();
        self.commit_state(
            State {
                base,
                pages,
                rev: 0,
                marks,
                fields,
            },
            change,
        );
    }

    fn commit_state(&mut self, mut state: State, change: Change) {
        let rev = self.next_rev;
        self.next_rev += 1;
        state.rev = rev;
        let old = std::mem::replace(&mut self.cur, state);
        self.undo.push((old, change));
        self.redo.clear();
        self.reindex();
    }

    pub fn undo(&mut self) -> Option<Change> {
        let (state, change) = self.undo.pop()?;
        let now = std::mem::replace(&mut self.cur, state);
        self.redo.push((now, change));
        self.reindex();
        Some(change)
    }

    pub fn redo(&mut self) -> Option<Change> {
        let (state, change) = self.redo.pop()?;
        let now = std::mem::replace(&mut self.cur, state);
        self.undo.push((now, change));
        self.reindex();
        Some(change)
    }

    /// Move the pages `sel` (in their order) to before page `to` (an
    /// index in the list as it is now; `len()` for the end). Returns where
    /// they are now, or `None` when nothing moves.
    pub fn move_pages(&mut self, sel: &BTreeSet<usize>, to: usize) -> Option<BTreeSet<usize>> {
        let sel: Vec<usize> = sel.iter().copied().filter(|&i| i < self.len()).collect();
        if sel.is_empty() {
            return None;
        }
        let moving: Vec<PageRef> = sel.iter().map(|&i| self.cur.pages[i]).collect();
        let to = to.min(self.len());
        let before = sel.iter().filter(|&&i| i < to).count();
        let mut rest: Vec<PageRef> = self
            .cur
            .pages
            .iter()
            .enumerate()
            .filter(|(i, _)| !sel.contains(i))
            .map(|(_, p)| *p)
            .collect();
        let at = to - before;
        rest.splice(at..at, moving);
        if rest == self.cur.pages {
            return None;
        }
        self.commit(self.cur.base, rest, Change::Move);
        Some((at..at + sel.len()).collect())
    }

    /// Delete the pages `sel`; `None` (nothing done) when that would leave
    /// no page.
    pub fn delete(&mut self, sel: &BTreeSet<usize>) -> Option<usize> {
        let keep: Vec<PageRef> = self
            .cur
            .pages
            .iter()
            .enumerate()
            .filter(|(i, _)| !sel.contains(i))
            .map(|(_, p)| *p)
            .collect();
        if keep.is_empty() || keep.len() == self.len() {
            return None;
        }
        let gone = self.len() - keep.len();
        self.commit(self.cur.base, keep, Change::Delete);
        Some(gone)
    }

    /// Turn the pages `sel` a quarter clockwise (`cw`) or counter-clockwise.
    pub fn rotate(&mut self, sel: &BTreeSet<usize>, cw: bool) -> bool {
        let mut pages = self.cur.pages.clone();
        let mut any = false;
        for &i in sel {
            if let Some(p) = pages.get_mut(i) {
                p.rot = if cw {
                    Turn(p.rot).cw()
                } else {
                    Turn(p.rot).ccw()
                }
                .0;
                any = true;
            }
        }
        if any {
            self.commit(self.cur.base, pages, Change::Rotate);
        }
        any
    }

    /// Insert `new` before page `at`. Returns where they are.
    pub fn insert(
        &mut self,
        at: usize,
        new: Vec<PageRef>,
        change: Change,
    ) -> Option<BTreeSet<usize>> {
        if new.is_empty() {
            return None;
        }
        let at = at.min(self.len());
        let n = new.len();
        let mut pages = self.cur.pages.clone();
        pages.splice(at..at, new);
        self.commit(self.cur.base, pages, change);
        Some((at..at + n).collect())
    }

    /// Copies of the pages `sel`, placed after the last of them. Returns
    /// where the copies are.
    pub fn duplicate(&mut self, sel: &BTreeSet<usize>) -> Option<BTreeSet<usize>> {
        let copies: Vec<PageRef> = sel.iter().filter_map(|&i| self.page(i)).collect();
        let last = *sel.iter().next_back()?;
        self.insert(last + 1, copies, Change::Duplicate)
    }

    /// Replace the whole document with all pages of `source` (the result of
    /// an operation that changed the pages' contents), which becomes the
    /// base.
    pub fn replace(&mut self, source: Arc<Source>, change: Change) {
        let pages = all_pages(&source);
        let id = source.id;
        self.add_source(source);
        // The edits are part of the new source's pages now.
        self.commit_state(
            State {
                base: id,
                pages,
                rev: 0,
                marks: Arc::default(),
                fields: Arc::default(),
            },
            change,
        );
    }

    // ── annotations and forms ────────────────────────────────────────────

    /// The annotations of source page `key` as they are now: edited, or
    /// the file's (`None` while the file's aren't read yet).
    pub fn marks(&self, key: PageKey) -> Option<Arc<Vec<Annot>>> {
        if let Some(m) = self.cur.marks.get(&key) {
            return Some(m.clone());
        }
        let s = self.sources.get(&key.0)?.marks_ready()?;
        s.pages.get(key.1).cloned()
    }

    /// Source pages whose annotations were changed.
    pub fn edited_keys(&self) -> impl Iterator<Item = PageKey> + '_ {
        self.cur.marks.keys().copied()
    }

    /// Every source's form values set.
    pub fn all_field_values(&self) -> Vec<(SrcId, Arc<HashMap<String, FieldValue>>)> {
        self.cur
            .fields
            .iter()
            .map(|(k, v)| (*k, v.clone()))
            .collect()
    }

    /// The pages are the base file's, in order, unturned (only
    /// annotations and form values may have changed).
    pub fn pages_as_base(&self) -> bool {
        let base = self.base();
        self.cur.pages.len() == base.sizes.len()
            && self
                .cur
                .pages
                .iter()
                .enumerate()
                .all(|(i, p)| p.src == base.id && p.page == i && p.rot == 0)
    }

    /// Replace source page `key`'s annotations (one undo step).
    pub fn set_marks(&mut self, key: PageKey, marks: Vec<Annot>, change: Change) {
        self.set_many_marks(vec![(key, marks)], change);
    }

    /// Replace several pages' annotations in one undo step.
    pub fn set_many_marks(&mut self, list: Vec<(PageKey, Vec<Annot>)>, change: Change) {
        let mut state = self.cur.clone();
        let mut all = (*state.marks).clone();
        for (key, marks) in list {
            all.insert(key, Arc::new(marks));
        }
        state.marks = Arc::new(all);
        self.commit_state(state, change);
    }

    /// Form values set in source `src` (over the file's).
    pub fn field_values(&self, src: SrcId) -> Option<&Arc<HashMap<String, FieldValue>>> {
        self.cur.fields.get(&src)
    }

    /// A field's value now.
    pub fn field_value(&self, src: SrcId, name: &str) -> Option<FieldValue> {
        if let Some(v) = self.cur.fields.get(&src).and_then(|f| f.get(name)) {
            return Some(v.clone());
        }
        let s = self.sources.get(&src)?.marks_ready()?;
        Some(s.form.as_ref()?.field(name)?.value.clone())
    }

    /// Set form values of source `src` (one undo step).
    pub fn set_fields(&mut self, src: SrcId, values: Vec<(String, FieldValue)>, change: Change) {
        let mut state = self.cur.clone();
        let mut all = (*state.fields).clone();
        let mut f = all.get(&src).map(|f| (**f).clone()).unwrap_or_default();
        f.extend(values);
        all.insert(src, Arc::new(f));
        state.fields = Arc::new(all);
        self.commit_state(state, change);
    }

    /// Whether any page has annotation edits or the form values set.
    pub fn has_markup_edits(&self) -> bool {
        !self.cur.marks.is_empty() || !self.cur.fields.is_empty()
    }

    /// Everything to build the PDF from.
    pub fn plan(&self) -> Plan {
        let (sources, picks) = self.picks();
        let marks = sources
            .iter()
            .map(|s| {
                self.cur
                    .marks
                    .iter()
                    .filter(|(k, _)| k.0 == s.id)
                    .map(|(k, v)| (k.1, v.clone()))
                    .collect()
            })
            .collect();
        let fields = sources
            .iter()
            .map(|s| self.cur.fields.get(&s.id).cloned())
            .collect();
        Plan {
            sources,
            picks,
            marks,
            fields,
            flatten: false,
            pages_as_base: self.pages_as_base(),
        }
    }

    /// The sources to build the PDF from (the base first) and the pages as
    /// `kantor_pdf` picks.
    pub fn picks(&self) -> (Vec<Arc<Source>>, Vec<kantor_pdf::Pick>) {
        let mut order: Vec<SrcId> = vec![self.cur.base];
        let mut picks = Vec::with_capacity(self.len());
        for p in &self.cur.pages {
            let source = match order.iter().position(|&s| s == p.src) {
                Some(i) => i,
                None => {
                    order.push(p.src);
                    order.len() - 1
                }
            };
            picks.push(kantor_pdf::Pick {
                source,
                page: p.page as u32,
                turn: i64::from(p.rot % 4) * 90,
            });
        }
        let sources = order.iter().map(|id| self.sources[id].clone()).collect();
        (sources, picks)
    }

    /// Whether the document is just its base file, unchanged (Save can
    /// keep the bytes as they are).
    pub fn is_base_as_is(&self) -> bool {
        let base = self.base();
        !self.has_markup_edits()
            && self.cur.pages.len() == base.sizes.len()
            && self
                .cur
                .pages
                .iter()
                .enumerate()
                .all(|(i, p)| p.src == base.id && p.page == i && p.rot == 0)
    }
}

fn all_pages(s: &Source) -> Vec<PageRef> {
    (0..s.sizes.len())
        .map(|page| PageRef {
            src: s.id,
            page,
            rot: 0,
        })
        .collect()
}

/// What the PDF is built from: the sources (the base first), the pages
/// picked, and each source's annotation edits and form values.
#[derive(Clone)]
pub(crate) struct Plan {
    pub sources: Vec<Arc<Source>>,
    pub picks: Vec<kantor_pdf::Pick>,
    /// By source index: page → its annotations.
    pub marks: Vec<HashMap<usize, Arc<Vec<Annot>>>>,
    pub fields: Vec<Option<Arc<HashMap<String, FieldValue>>>>,
    /// Draw stamps and signatures into the pages.
    pub flatten: bool,
    /// Only annotations or form values changed (a signed file can get an
    /// incremental update).
    pub pages_as_base: bool,
}

impl Plan {
    /// Source `i`'s annotation edits and form values, applied to `pdf`.
    fn apply(&self, i: usize, pdf: &mut kantor_pdf::Pdf) -> kantor_pdf::Result<()> {
        if let Some(Some(f)) = self.fields.get(i) {
            kantor_pdf::form::set_values(pdf, f)?;
        }
        if let Some(m) = self.marks.get(i) {
            for (&page, marks) in m {
                kantor_pdf::annot::apply_page(pdf, page as u32, marks)?;
            }
        }
        Ok(())
    }

    /// The base file is digitally signed and only its annotations or form
    /// values changed: saved as an incremental update that keeps the
    /// signatures valid.
    pub fn incremental(&self) -> bool {
        self.pages_as_base
            && !self.flatten
            && self.sources.len() == 1
            && self.sources[0].marks().is_some_and(|m| m.sigs.is_some())
    }

    /// The sources as `kantor_pdf` documents with the edits applied.
    pub fn load(&self) -> kantor_pdf::Result<Vec<kantor_pdf::Pdf>> {
        let mut out = Vec::with_capacity(self.sources.len());
        for (i, s) in self.sources.iter().enumerate() {
            let mut pdf = s.lopdf()?;
            self.apply(i, &mut pdf)?;
            if self.flatten {
                kantor_pdf::annot::flatten(
                    &mut pdf,
                    kantor_pdf::annot::FlattenWhat::StampsAndSignatures,
                )?;
            }
            out.push(pdf);
        }
        Ok(out)
    }

    /// The pages (and their sources) as one PDF.
    pub fn assemble(&self) -> kantor_pdf::Result<kantor_pdf::Pdf> {
        kantor_pdf::assemble(self.load()?, &self.picks)
    }

    /// Only the pages `which` (indexes into the picks).
    pub fn only(&self, which: &[usize]) -> Plan {
        let mut p = self.clone();
        p.picks = which
            .iter()
            .filter_map(|&i| self.picks.get(i).copied())
            .collect();
        p
    }
}

/// Build the edited PDF's bytes (on a background thread). `keep_encryption`:
/// when the base is password-protected, protect the result the same way;
/// fails with `kantor_pdf::Error::Encryption` when that isn't possible.
pub(crate) fn build(plan: &Plan, keep_encryption: bool) -> kantor_pdf::Result<Vec<u8>> {
    if plan.incremental() {
        let base = &plan.sources[0];
        let password = (!base.password.is_empty()).then_some(base.password.as_str());
        return kantor_pdf::sig::incremental(&base.bytes, password, |pdf| plan.apply(0, pdf));
    }
    let mut pdf = plan.assemble()?;
    pdf.to_bytes_encrypted(keep_encryption)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(n: usize) -> Arc<Source> {
        Arc::new(Source {
            id: next_id(),
            name: "t.pdf".into(),
            bytes: Arc::new(Vec::new()),
            password: String::new(),
            sizes: (0..n).map(|i| (600.0 + i as f32, 800.0)).collect(),
            outline: Vec::new(),
            title: None,
            xf: Vec::new(),
            marks: OnceLock::new(),
        })
    }

    fn order(m: &Model) -> Vec<usize> {
        m.pages().iter().map(|p| p.page).collect()
    }

    fn set(v: &[usize]) -> BTreeSet<usize> {
        v.iter().copied().collect()
    }

    #[std::prelude::v1::test]
    fn moves_pages_before_a_place() {
        let mut m = Model::new(source(6));
        // Pages 2 and 4 (0-based 1, 3) to the front.
        assert_eq!(m.move_pages(&set(&[1, 3]), 0), Some(set(&[0, 1])));
        assert_eq!(order(&m), [1, 3, 0, 2, 4, 5]);
        // To the end.
        assert_eq!(m.move_pages(&set(&[0]), 6), Some(set(&[5])));
        assert_eq!(order(&m), [3, 0, 2, 4, 5, 1]);
        // Down past itself: the gap counts in the list as it was.
        assert_eq!(m.move_pages(&set(&[1, 2]), 4), Some(set(&[2, 3])));
        assert_eq!(order(&m), [3, 4, 0, 2, 5, 1]);
        // Onto its own place: nothing.
        assert_eq!(m.move_pages(&set(&[2]), 3), None);
        assert_eq!(m.move_pages(&set(&[2]), 2), None);
        assert!(m.is_dirty());
    }

    #[std::prelude::v1::test]
    fn delete_rotate_duplicate_undo_redo() {
        let s = source(4);
        let mut m = Model::new(s.clone());
        assert!(!m.is_dirty());
        assert!(m.is_base_as_is());
        assert_eq!(m.delete(&set(&[0, 1, 2, 3])), None, "keeps one page");
        assert_eq!(m.delete(&set(&[1])), Some(1));
        assert_eq!(order(&m), [0, 2, 3]);
        assert!(m.rotate(&set(&[0]), true));
        assert_eq!(m.size(0), (800.0, 600.0));
        assert!(m.rotate(&set(&[0]), false));
        assert!(m.rotate(&set(&[0]), false));
        assert_eq!(m.page(0).unwrap().rot, 3);
        assert_eq!(m.duplicate(&set(&[0, 1])), Some(set(&[2, 3])));
        assert_eq!(order(&m), [0, 2, 0, 2, 3]);
        assert_eq!(m.index_of((s.id, 0)), Some(0));
        assert_eq!(m.index_of((s.id, 1)), None);
        assert_eq!(m.undo(), Some(Change::Duplicate));
        assert_eq!(m.undo(), Some(Change::Rotate));
        assert_eq!(m.redo_change(), Some(Change::Rotate));
        assert_eq!(m.redo(), Some(Change::Rotate));
        assert_eq!(m.undo(), Some(Change::Rotate));
        assert_eq!(m.undo(), Some(Change::Rotate));
        assert_eq!(m.undo(), Some(Change::Rotate));
        assert_eq!(m.undo(), Some(Change::Delete));
        assert_eq!(m.undo(), None);
        assert!(!m.is_dirty(), "undone back to the saved state");
        assert_eq!(m.redo(), Some(Change::Delete));
        assert!(m.is_dirty());
        m.mark_saved(m.revision());
        assert!(!m.is_dirty());
        // A new change after an undo drops the redo steps.
        m.undo();
        assert!(m.is_dirty());
        m.rotate(&set(&[1]), true);
        assert_eq!(m.redo(), None);
    }

    #[std::prelude::v1::test]
    fn inserts_other_sources_and_picks_them() {
        let a = source(3);
        let b = source(2);
        let mut m = Model::new(a.clone());
        m.add_source(b.clone());
        let new = vec![
            PageRef {
                src: b.id,
                page: 1,
                rot: 0,
            },
            PageRef {
                src: b.id,
                page: 0,
                rot: 1,
            },
        ];
        assert_eq!(m.insert(1, new, Change::Insert), Some(set(&[1, 2])));
        assert_eq!(m.size(2), (800.0, 600.0));
        m.delete(&set(&[0]));
        let (sources, picks) = m.picks();
        // The base comes first even when its first page is gone.
        assert_eq!(sources[0].id, a.id);
        assert_eq!(sources[1].id, b.id);
        let p: Vec<(usize, u32, i64)> = picks.iter().map(|p| (p.source, p.page, p.turn)).collect();
        assert_eq!(p, [(1, 1, 0), (1, 0, 90), (0, 1, 0), (0, 2, 0)]);
        // Replacing makes a new base; undo brings the old one back.
        let c = source(2);
        m.replace(c.clone(), Change::Watermark);
        assert_eq!(m.base().id, c.id);
        assert_eq!(m.len(), 2);
        m.undo();
        assert_eq!(m.base().id, a.id);
        assert_eq!(m.len(), 4);
    }
}

/// Edits on real PDFs, saved and opened again with hayro (and checked with
/// qpdf when it is installed).
#[cfg(test)]
mod save_tests {
    use super::*;
    use kantor_pdf::testing::{sample_pdf, sample_pdf_encrypted, sample_pdf_linked};

    fn read(bytes: Vec<u8>, password: &str) -> Arc<Source> {
        Arc::new(Source::read("t.pdf".into(), Arc::new(bytes), password.into()).unwrap())
    }

    fn set(v: &[usize]) -> BTreeSet<usize> {
        v.iter().copied().collect()
    }

    /// Widths of the saved pages (sample page k is 600 + k wide).
    fn widths(bytes: Vec<u8>, password: &str) -> Vec<u32> {
        let doc = PdfDoc::open(Arc::new(bytes), password).unwrap();
        doc.page_sizes()
            .iter()
            .map(|&(w, _)| w as u32 - 600)
            .collect()
    }

    fn save(m: &Model) -> Vec<u8> {
        let bytes = build(&m.plan(), true).unwrap();
        qpdf_check(&bytes, None);
        bytes
    }

    fn qpdf_check(bytes: &[u8], password: Option<&str>) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("c.pdf");
        std::fs::write(&path, bytes).unwrap();
        let mut cmd = std::process::Command::new("qpdf");
        if let Some(p) = password {
            cmd.arg(format!("--password={p}"));
        }
        if let Ok(out) = cmd.arg("--check").arg(&path).output() {
            assert!(
                out.status.success(),
                "qpdf: {}",
                String::from_utf8_lossy(&out.stdout)
            );
        }
    }

    #[std::prelude::v1::test]
    fn every_page_operation_saves_and_reopens() {
        let a = read(sample_pdf(5), "");
        let b = read(sample_pdf(2), "");
        let mut m = Model::new(a.clone());
        m.add_source(b.clone());
        m.move_pages(&set(&[4]), 0);
        m.delete(&set(&[1]));
        m.rotate(&set(&[0]), true);
        m.duplicate(&set(&[0]));
        let other = PageRef {
            src: b.id,
            page: 1,
            rot: 0,
        };
        m.insert(2, vec![other], Change::Paste);
        let blank = kantor_pdf::blank_page(300.0, 400.0).to_bytes().unwrap();
        let blank = read(blank, "");
        m.add_source(blank.clone());
        let bp = PageRef {
            src: blank.id,
            page: 0,
            rot: 0,
        };
        m.insert(m.len(), vec![bp], Change::Insert);
        // 5 (turned), 5 copy, page 2 of b, 2, 3, 4, blank.
        let bytes = save(&m);
        let doc = PdfDoc::open(Arc::new(bytes), "").unwrap();
        let sizes = doc.page_sizes();
        assert_eq!(sizes.len(), 7);
        assert_eq!(sizes[0], (800.0, 605.0), "rotated a quarter");
        assert_eq!(sizes[1], (800.0, 605.0), "the copy keeps the turn");
        assert_eq!(sizes[2], (602.0, 800.0));
        assert_eq!(sizes[6], (300.0, 400.0));
        assert!(doc.text(2).text.contains("Page 2"));
        // Undo everything: the saved file is the original again.
        while m.undo().is_some() {}
        assert!(m.is_base_as_is());
        assert_eq!(widths(save(&m), ""), [1, 2, 3, 4, 5]);
    }

    #[std::prelude::v1::test]
    fn protected_files_stay_protected() {
        let a = read(sample_pdf_encrypted(3, "owner", "user"), "user");
        let mut m = Model::new(a);
        m.move_pages(&set(&[2]), 0);
        let bytes = build(&m.plan(), true).unwrap();
        assert!(PdfDoc::open(Arc::new(bytes.clone()), "").is_err());
        assert_eq!(widths(bytes.clone(), "user"), [3, 1, 2]);
        qpdf_check(&bytes, Some("user"));
    }

    #[std::prelude::v1::test]
    fn bookmarks_follow_their_pages() {
        let a = read(sample_pdf_linked(3), "");
        let mut m = Model::new(a.clone());
        m.move_pages(&set(&[2]), 0);
        m.delete(&set(&[2]));
        // Page 2 of the file is gone; page 3 is first.
        assert_eq!(m.index_of((a.id, 2)), Some(0));
        assert_eq!(m.index_of((a.id, 1)), None);
        let bytes = save(&m);
        let doc = PdfDoc::open(Arc::new(bytes), "").unwrap();
        let titles: Vec<String> = doc.outline().iter().map(|o| o.title.clone()).collect();
        assert_eq!(titles, ["Page 1", "Page 3"]);
    }

    /// 600 pages: moving and deleting are instant; saving takes a moment.
    #[std::prelude::v1::test]
    fn big_file_edits_and_saves_quickly() {
        // KANTOR_PDF_BIG=file.pdf times a real long file (e.g. 600 pages of
        // text exported by Kantor) instead of the tiny generated one.
        let bytes = match std::env::var_os("KANTOR_PDF_BIG") {
            Some(p) => std::fs::read(p).unwrap(),
            None => sample_pdf(600),
        };
        let a = read(bytes, "");
        assert!(a.sizes.len() >= 600);
        let n = a.sizes.len();
        let mut m = Model::new(a);
        let t = std::time::Instant::now();
        m.move_pages(&(0..300).step_by(2).collect(), n);
        m.delete(&(100..400).collect());
        m.rotate(&(0..300).collect(), false);
        let edit = t.elapsed();
        let t = std::time::Instant::now();
        let bytes = build(&m.plan(), true).unwrap();
        let saved = t.elapsed();
        eprintln!(
            "{n} pages: edits {edit:.1?}, save {saved:.1?} ({} KB)",
            bytes.len() / 1024
        );
        assert!(edit.as_millis() < 200, "edits took {edit:?}");
        assert!(saved.as_secs() < 20, "save took {saved:?}");
        let doc = PdfDoc::open(Arc::new(bytes.clone()), "").unwrap();
        assert_eq!(doc.page_count(), a_len(&m));
        qpdf_check(&bytes, None);
    }

    fn annots_of(bytes: &[u8], page: u32) -> Vec<kantor_pdf::annot::Annot> {
        let pdf = kantor_pdf::Pdf::from_bytes(bytes, None).unwrap();
        kantor_pdf::annot::read_page(&pdf, page)
    }

    #[std::prelude::v1::test]
    fn annotations_and_forms_save_and_undo() {
        use kantor_pdf::annot::{Annot, Kind};
        use kantor_pdf::form::FieldValue;
        let a = read(sample_pdf(3), "");
        let mut m = Model::new(a.clone());
        assert!(a.marks().is_some());
        assert_eq!(m.marks((a.id, 1)).unwrap().len(), 0);
        let note = Annot::new(Kind::Note, [10.0, 10.0, 30.0, 30.0], [1.0, 0.8, 0.0]);
        m.set_marks(
            (a.id, 1),
            vec![note.clone()],
            Change::Markup("Add Sticky Note"),
        );
        assert!(m.is_dirty() && !m.is_base_as_is());
        // Moved first: the annotation goes with its page.
        m.move_pages(&set(&[1]), 0);
        let bytes = save(&m);
        let back = annots_of(&bytes, 0);
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].id, note.id);
        assert!(annots_of(&bytes, 1).is_empty());
        // Undo both: the original file again.
        assert_eq!(m.undo(), Some(Change::Move));
        assert_eq!(m.undo(), Some(Change::Markup("Add Sticky Note")));
        assert!(m.is_base_as_is());
        assert_eq!(m.marks((a.id, 1)).unwrap().len(), 0);
        // Forms.
        let f = read(kantor_pdf::testing::sample_form_pdf(), "");
        let mut m = Model::new(f.clone());
        f.marks();
        assert_eq!(
            m.field_value(f.id, "name"),
            Some(FieldValue::Text("Ann".into()))
        );
        m.set_fields(
            f.id,
            vec![("name".into(), FieldValue::Text("Rina".into()))],
            Change::Markup("Fill Form"),
        );
        assert_eq!(
            m.field_value(f.id, "name"),
            Some(FieldValue::Text("Rina".into()))
        );
        let bytes = save(&m);
        let form = kantor_pdf::form::read_form(&kantor_pdf::Pdf::from_bytes(&bytes, None).unwrap())
            .unwrap();
        assert_eq!(
            form.field("name").unwrap().value,
            FieldValue::Text("Rina".into())
        );
        m.undo();
        assert_eq!(
            m.field_value(f.id, "name"),
            Some(FieldValue::Text("Ann".into()))
        );
    }

    #[std::prelude::v1::test]
    fn signed_files_keep_their_signed_bytes() {
        use kantor_pdf::annot::{Annot, Art, Kind};
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../kantor_pdf/tests/fixtures/signed/signed-approval.pdf");
        let original = std::fs::read(path).unwrap();
        let s = read(original.clone(), "");
        assert!(s.marks().unwrap().sigs.is_some());
        let mut m = Model::new(s.clone());
        let stamp = Annot::new(
            Kind::Stamp {
                art: Art::Image(Arc::new(kantor_pdf::testing::sample_round_stamp(32))),
                rotate: 0.0,
                signature: false,
            },
            [100.0, 100.0, 200.0, 200.0],
            [0.0; 3],
        );
        let mut list = (*m.marks((s.id, 0)).unwrap()).clone();
        list.push(stamp);
        list.push(Annot::new(
            Kind::Note,
            [10.0, 10.0, 30.0, 30.0],
            [1.0, 0.8, 0.0],
        ));
        m.set_marks((s.id, 0), list, Change::Markup("Add Stamp"));
        let plan = m.plan();
        assert!(plan.incremental());
        let bytes = build(&plan, true).unwrap();
        assert!(bytes.starts_with(&original), "incremental update");
        qpdf_check(&bytes, None);
        // Flattened or with pages changed: a full rewrite (a copy).
        let mut flat = m.plan();
        flat.flatten = true;
        assert!(!flat.incremental());
        m.rotate(&set(&[0]), true);
        assert!(!m.plan().incremental());
    }

    fn a_len(m: &Model) -> usize {
        m.len()
    }
}
