//! The whole page layout: body text flowed through its sections, and each
//! page's header and footer with its page numbers filled in.
//!
//! Word's rules followed here:
//! - Each section has its own paper, orientation and margins; a section
//!   starts on a new page unless it's continuous (and the same size).
//! - The header sits `w:header` from the top edge, the footer `w:footer`
//!   from the bottom; a header or footer taller than its margin pushes the
//!   body text in (the text area shrinks; nothing overlaps).
//! - "Different first page" uses the first header/footer on the section's
//!   first page (blank when it has none); "different odd and even" uses the
//!   even ones on even-numbered pages. A section without its own uses the
//!   previous section's ("Link to Previous").
//! - `PAGE` shows the page's number in the section's format, counting on
//!   from the previous section unless the section restarts it; `NUMPAGES`
//!   shows the number of pages.
//! - Footnotes sit at the bottom of the page whose text holds their
//!   reference, under a short separator line; the text area shrinks to make
//!   room. A note too long for what's left of its page starts there and
//!   goes on at the bottom of the next page, under a full-width
//!   continuation separator (and the document's continuation notice under
//!   the part that goes on, if it has one). Endnotes follow the end of the
//!   document, or of each section when the document says so (`w:pos
//!   sectEnd`), and break across pages at their lines. Numbers restart per
//!   section or per page as the document's footnote settings say.

use std::collections::HashMap;
use std::sync::Arc;

use docs_engine::{
    Document, HfKind, HfType, NoteKind, NoteRef, NoteRestart, Section, SectionStart,
};
use docs_layout::{
    Block, NOTE_SEPARATOR, ObjectKind, PageSetup, PagedDocument, ParaLayout, Rgba, RuleRect,
    SectionBreak, SectionFlow,
};

use crate::build::{self, Marks};
use crate::floats::{self, FloatItem};

/// Lays out one paragraph broken at a width (the Docs view passes its
/// cache, export the engine).
pub type LayoutFn<'a> = dyn FnMut(&docs_layout::Paragraph, f32) -> Arc<ParaLayout> + 'a;

/// A header, footer or note laid out for some page.
pub struct StoryLayout {
    /// Index into [`Document::stories`] (headers and footers).
    pub story: usize,
    /// Index into [`Document::notes`] (footnotes and endnotes).
    pub note: Option<usize>,
    /// A text box: its object's header or footer story (`None`: the
    /// body), paragraph and byte index.
    pub text_box: Option<(Option<usize>, usize, usize)>,
    /// With [`StoryLayout::text_box`]: the text of the group's shape
    /// number `n` ([`docs_engine::Float::boxes`]) rather than the
    /// object's own text box.
    pub group_box: Option<usize>,
    /// Laid out on one tall page whose content area starts at 0, 0.
    pub paged: PagedDocument,
    /// The story's paragraphs as laid out (page numbers filled in).
    pub paras: Vec<docs_engine::Paragraph>,
    /// Height of its content, px.
    pub height: f32,
    /// The layout paragraphs, kept only when some text is struck through
    /// (tracked deletions).
    pub strikes: Arc<Vec<docs_layout::Paragraph>>,
}

impl StoryLayout {
    /// Strikethrough lines for its tracked deletions (its own coordinates).
    pub fn strike_rules(&self) -> Vec<RuleRect> {
        if self.strikes.is_empty() {
            return Vec::new();
        }
        self.paged.strike_rules(0, &self.strikes)
    }

    pub fn text(&self, para: usize) -> &str {
        self.paras.get(para).map_or("", |p| p.text.as_str())
    }

    /// A paragraph's text as shown: page numbers and note numbers in
    /// place of their objects, other objects left out.
    pub fn shown(&self, para: usize) -> String {
        let text = self.text(para);
        let labels = self
            .paged
            .layouts
            .get(para)
            .map_or(&[][..], |l| &l.labels[..]);
        let mut out = String::with_capacity(text.len());
        for (i, c) in text.char_indices() {
            if c != docs_engine::OBJECT {
                out.push(c);
            } else if let Some((_, _, label)) = labels.iter().find(|x| x.0 == i) {
                out.push_str(label);
            }
        }
        out
    }

    /// The small document laid out (for its pictures).
    pub fn doc<'a>(&self, doc: &'a Document) -> &'a Document {
        if let Some((story, para, index)) = self.text_box {
            let owner = match story {
                Some(s) => doc.stories().get(s).map_or(doc, |st| &st.doc),
                None => doc,
            };
            return owner
                .float_at(para, index)
                .and_then(|f| f.text_box_in(self.group_box))
                .map_or(owner, |t| &t.doc);
        }
        match self.note {
            Some(i) => &doc.notes()[i].doc,
            None => &doc.stories()[self.story].doc,
        }
    }

    /// A key telling stories apart (picture caches).
    pub fn key(&self) -> usize {
        if let Some((story, para, index)) = self.text_box {
            let sub = self.group_box.map_or(0, |k| (k + 1) << 48);
            return sub + (3 << 40) + (story.map_or(0, |s| s + 1) << 32) + (para << 12) + index;
        }
        match self.note {
            Some(i) => (1 << 24) + i,
            None => self.story,
        }
    }
}

/// A header or footer placed on a page: its layout's (0, 0) goes to
/// (`x`, `y`) in page coordinates.
#[derive(Clone)]
pub struct Placed {
    pub kind: HfKind,
    pub layout: Arc<StoryLayout>,
    pub x: f32,
    pub y: f32,
    /// A footnote or endnote (drawn in full ink, edited on double click).
    pub note: Option<(NoteKind, String)>,
}

impl Placed {
    /// Its area on the page: `[x, y, w, h]`.
    pub fn rect(&self) -> [f32; 4] {
        [
            self.x,
            self.y,
            self.layout.paged.setup.width,
            self.layout.height,
        ]
    }
}

/// The laid-out document: body pages and what each page's header and
/// footer show.
pub struct Layout {
    pub paged: PagedDocument,
    /// Per page: its header and/or footer.
    pub furniture: Vec<Vec<Placed>>,
    /// Per page: its floating objects (body, header and footer), in
    /// z-order (lowest first).
    pub floats: Vec<Vec<FloatItem>>,
    /// The layout paragraphs, kept only when some text is struck through
    /// (tracked deletions; see [`PagedDocument::strike_rules`]).
    pub strikes: Arc<Vec<docs_layout::Paragraph>>,
}

impl Layout {
    /// Strikethrough lines on a page.
    pub fn strike_rules(&self, page: usize) -> Vec<docs_layout::RuleRect> {
        if self.strikes.is_empty() {
            return Vec::new();
        }
        self.paged.strike_rules(page, &self.strikes)
    }
}

fn twips(t: u32) -> f32 {
    t as f32 / 15.0
}

/// A story laid out at `width` px with `page` / `pages` / section pages
/// as its numbers.
fn story_layout(
    doc: &Document,
    index: usize,
    width: f32,
    (page, pages, section_pages): (&str, &str, &str),
    style_refs: &[String],
    layout: &mut LayoutFn,
) -> StoryLayout {
    let sd = &doc.stories()[index].doc;
    // STYLEREF fields show what the page has ([`docs_engine::style_ref_results`]).
    let paras = if style_refs.is_empty() {
        sd.paragraphs().to_vec()
    } else {
        docs_engine::with_style_ref_results(sd.paragraphs(), style_refs)
    };
    let marks = Marks {
        pages: Some([page.to_owned(), pages.to_owned(), section_pages.to_owned()]),
        codes: doc.field_codes().cloned(),
        merge: doc.merge_view().cloned(),
        ..Marks::default()
    };
    lay_story(sd, paras, width, &marks, layout, index, None)
}

/// A footnote or endnote laid out at `width` px, numbered `label`.
fn note_layout(
    doc: &Document,
    index: usize,
    width: f32,
    label: &str,
    layout: &mut LayoutFn,
) -> StoryLayout {
    let nd = &doc.notes()[index].doc;
    let marks = Marks {
        own: Some(label.to_owned()),
        codes: doc.field_codes().cloned(),
        merge: doc.merge_view().cloned(),
        ..Marks::default()
    };
    lay_story(
        nd,
        nd.paragraphs().to_vec(),
        width,
        &marks,
        layout,
        usize::MAX,
        Some(index),
    )
}

/// Paragraphs of a small document laid out on one tall page `width` px
/// wide.
fn lay_story(
    sd: &Document,
    paras: Vec<docs_engine::Paragraph>,
    width: f32,
    marks: &Marks,
    layout: &mut LayoutFn,
    story: usize,
    note: Option<usize>,
) -> StoryLayout {
    let width_tw = (width * 15.0).round() as u32;
    let marks = Marks {
        no_floats: true,
        ..marks.clone()
    };
    let (lps, widths, blocks) = build::build_with(sd, &paras, &|_| width_tw, &marks);
    let layouts: Vec<Arc<ParaLayout>> = lps
        .iter()
        .zip(&widths)
        .map(|(p, &w)| layout(p, w))
        .collect();
    let setup = PageSetup {
        width,
        height: 1.0e6,
        margin_top: 0.0,
        margin_bottom: 0.0,
        margin_left: 0.0,
        margin_right: 0.0,
    };
    let paged = PagedDocument::paginate_blocks(&lps, layouts, &blocks, setup);
    let mut height = 0.0f32;
    if let Some(p) = paged.pages.first() {
        for l in &p.lines {
            height = height.max(l.top + l.height);
        }
        for r in &p.rules {
            height = height.max(r.rect[1] + r.rect[3]);
        }
    }
    // The last paragraph's space after counts, as in Word.
    if let Some(last) = lps.last() {
        height += last.space_after_pt * docs_layout::PT;
    }
    let strikes = if lps.iter().any(|p| !p.strikes.is_empty()) {
        Arc::new(lps)
    } else {
        Arc::default()
    };
    StoryLayout {
        strikes,
        story,
        note,
        text_box: None,
        group_box: None,
        paged,
        paras,
        height,
    }
}

/// The stories a section's pages use: (header, footer) for default, first
/// and even pages.
struct SectionStories {
    default: (Option<usize>, Option<usize>),
    first: (Option<usize>, Option<usize>),
    even: (Option<usize>, Option<usize>),
}

fn section_stories(doc: &Document, sections: &[Section], si: usize) -> SectionStories {
    let pick = |ty| {
        (
            doc.story_index_for(sections, si, HfKind::Header, ty),
            doc.story_index_for(sections, si, HfKind::Footer, ty),
        )
    };
    let props = &sections[si].props;
    SectionStories {
        default: pick(HfType::Default),
        first: if props.title_page {
            pick(HfType::First)
        } else {
            pick(HfType::Default)
        },
        even: if doc.even_odd_headers() {
            pick(HfType::Even)
        } else {
            pick(HfType::Default)
        },
    }
}

/// Footnotes laid out once per (note, number, width).
type NoteCache = HashMap<(usize, String, u32), Arc<StoryLayout>>;

fn cached_note(
    cache: &mut NoteCache,
    doc: &Document,
    index: usize,
    width: f32,
    label: &str,
    layout: &mut LayoutFn,
) -> Arc<StoryLayout> {
    cache
        .entry((index, label.to_owned(), width.to_bits()))
        .or_insert_with(|| Arc::new(note_layout(doc, index, width, label, layout)))
        .clone()
}

/// The body's layout paragraphs with their footnotes' heights, numbered
/// with `refs`' labels.
#[allow(clippy::type_complexity)]
fn body_with_notes(
    doc: &Document,
    refs: &[NoteRef],
    sections: &[Section],
    cache: &mut NoteCache,
    layout: &mut LayoutFn,
    cut: usize,
    prior: Option<build::Prior>,
) -> build::Built {
    let marks = Marks {
        refs: refs
            .iter()
            .map(|r| ((r.kind, r.id.clone()), r.label.clone()))
            .collect(),
        own: None,
        no_floats: false,
        pages: None,
        codes: doc.field_codes().cloned(),
        merge: doc.merge_view().cloned(),
    };
    let mut built = build::build_body(doc, &marks, cut, prior);
    let lps = &mut built.lps;
    for r in refs.iter().filter(|r| r.kind == NoteKind::Footnote) {
        // Paragraphs not built again have their notes.
        if !built.fresh.get(r.pos.para).copied().unwrap_or(true) {
            continue;
        }
        let Some(i) = doc.note_index(r.kind, &r.id) else {
            continue;
        };
        let width = section_width(sections, r.section);
        let l = cached_note(cache, doc, i, width, &r.label, layout);
        let notice =
            notice_layout(doc, NoteKind::Footnote, width, cache, layout).map_or(0.0, |n| n.height);
        let Some(lp) = lps.get_mut(r.pos.para) else {
            continue;
        };
        lp.notes.push((r.pos.offset, l.height));
        lp.note_splits.push(docs_layout::NoteSplit {
            index: r.pos.offset,
            breaks: breaks(&l),
            notice,
        });
        // Columns of other widths: the note laid out at each (Word puts
        // a note under its column, as wide as it).
        let mut others: Vec<f32> = sections
            .get(r.section)
            .map(section_columns)
            .unwrap_or_default()
            .into_iter()
            .map(|c| c.1)
            .filter(|w| (w - width).abs() > 0.5)
            .collect();
        others.sort_by(f32::total_cmp);
        others.dedup_by(|a, b| (*a - *b).abs() < 0.5);
        for w in others {
            let l = cached_note(cache, doc, i, w, &r.label, layout);
            let notice =
                notice_layout(doc, NoteKind::Footnote, w, cache, layout).map_or(0.0, |n| n.height);
            let at = match lp
                .note_widths
                .iter()
                .position(|n| (n.width - w).abs() < 0.5)
            {
                Some(at) => at,
                None => {
                    lp.note_widths.push(docs_layout::NoteWidth {
                        width: w,
                        ..Default::default()
                    });
                    lp.note_widths.len() - 1
                }
            };
            let nw = &mut lp.note_widths[at];
            nw.notes.push((r.pos.offset, l.height));
            nw.splits.push(docs_layout::NoteSplit {
                index: r.pos.offset,
                breaks: breaks(&l),
                notice,
            });
        }
    }
    built
}

/// Where a laid-out note may break across pages: its lines' bottoms, then
/// its whole height.
fn breaks(l: &StoryLayout) -> Vec<f32> {
    let mut out: Vec<f32> = l
        .paged
        .pages
        .first()
        .map(|p| p.lines.iter().map(|x| x.top + x.height).collect())
        .unwrap_or_default();
    out.retain(|b| *b > 0.01 && *b < l.height - 0.01);
    out.sort_by(f32::total_cmp);
    out.dedup_by(|a, b| (*a - *b).abs() < 0.01);
    out.push(l.height.max(0.01));
    out
}

/// The continuation notice of `kind` laid out at `width`, if the document
/// has one with text.
fn notice_layout(
    doc: &Document,
    kind: NoteKind,
    width: f32,
    cache: &mut NoteCache,
    layout: &mut LayoutFn,
) -> Option<Arc<StoryLayout>> {
    let i = doc.continuation_notice(kind)?;
    Some(cached_note(cache, doc, i, width, "", layout))
}

/// Gap ids of endnote separators (others index the endnote references).
const SEPARATOR_GAP: usize = usize::MAX;

/// The body's blocks with endnotes in the flow: at the end of each
/// section whose endnotes go there (Word's "End of section"), the rest at
/// the end of the document; a separator line before each group. `flows`'
/// first blocks move with the inserted blocks.
fn with_endnotes(
    doc: &Document,
    refs: &[NoteRef],
    sections: &[Section],
    blocks: Vec<Block>,
    flows: &[SectionFlow],
    cache: &mut NoteCache,
    layout: &mut LayoutFn,
) -> (Vec<Block>, Vec<SectionFlow>) {
    let ends: Vec<(usize, &NoteRef)> = refs
        .iter()
        .enumerate()
        .filter(|(_, r)| r.kind == NoteKind::Endnote && doc.note(r.kind, &r.id).is_some())
        .collect();
    if ends.is_empty() {
        return (blocks, flows.to_vec());
    }
    let last = sections.len().saturating_sub(1);
    // Section whose end each endnote goes to.
    let target = |r: &NoteRef| {
        if doc
            .note_props_in(sections, NoteKind::Endnote, r.section)
            .section_end
        {
            r.section.min(last)
        } else {
            last
        }
    };
    let mut groups: Vec<Vec<Block>> = vec![Vec::new(); sections.len().max(1)];
    for (k, r) in &ends {
        let si = target(r);
        let Some(i) = doc.note_index(r.kind, &r.id) else {
            continue;
        };
        let l = cached_note(
            cache,
            doc,
            i,
            section_width(sections, r.section),
            &r.label,
            layout,
        );
        let g = &mut groups[si];
        if g.is_empty() {
            g.push(Block::Gap(docs_layout::GapBlock {
                id: SEPARATOR_GAP,
                breaks: vec![NOTE_SEPARATOR],
                before: 12.0,
            }));
        }
        g.push(Block::Gap(docs_layout::GapBlock {
            id: *k,
            breaks: breaks(&l),
            before: 0.0,
        }));
    }
    let mut out = Vec::with_capacity(blocks.len() + ends.len() * 2);
    let mut out_flows = flows.to_vec();
    let mut blocks = blocks.into_iter().enumerate().peekable();
    for (si, group) in groups.into_iter().enumerate() {
        let end = flows.get(si + 1).map_or(usize::MAX, |f| f.first_block);
        while let Some((_, b)) = blocks.next_if(|(bi, _)| *bi < end) {
            out.push(b);
        }
        out.extend(group);
        if let Some(f) = out_flows.get_mut(si + 1) {
            f.first_block = out.len();
        }
    }
    out.extend(blocks.map(|(_, b)| b));
    (out, out_flows)
}

/// A section's text width, px: its first column's when it has columns
/// (footnotes and endnotes are as wide as their column).
fn section_width(sections: &[Section], si: usize) -> f32 {
    sections.get(si).map_or(
        PageSetup::a4().content_width(),
        |s| match section_columns(s).first() {
            Some(&(_, w)) => w,
            None => build::geometry_setup(&s.props.page).content_width(),
        },
    )
}

/// A section's columns (left edge from the margin, width; px), empty
/// for one column.
pub fn section_columns(s: &Section) -> Vec<(f32, f32)> {
    if s.props.columns.count() <= 1 {
        return Vec::new();
    }
    let g = s.props.page;
    let text = g
        .width
        .saturating_sub(g.margin_left + g.margin_right)
        .max(1440);
    s.props
        .columns
        .layout(text)
        .into_iter()
        .map(|(x, w)| (twips(x), twips(w)))
        .collect()
}

/// Footnote numbers restarted on each page (sections set to "each page"),
/// from where the references landed.
fn renumber_per_page(
    doc: &Document,
    sections: &[Section],
    refs: &[NoteRef],
    paged: &PagedDocument,
) -> Vec<NoteRef> {
    let mut out = refs.to_vec();
    let mut count: HashMap<usize, u32> = HashMap::new();
    let mut props_of: HashMap<usize, docs_engine::NoteProps> = HashMap::new();
    for r in &mut out {
        if r.kind != NoteKind::Footnote || r.label.is_empty() {
            continue;
        }
        let props = *props_of
            .entry(r.section)
            .or_insert_with(|| doc.note_props_in(sections, r.kind, r.section));
        if props.restart != NoteRestart::EachPage {
            continue;
        }
        let page = paged.caret(r.pos.para, r.pos.offset).map_or(0, |c| c.page);
        let n = count.entry(page).or_insert(0);
        r.label = props.format.format(props.start + *n);
        *n += 1;
    }
    out
}

/// Separator line above notes, starting at `x`, `y` (its band's top).
fn separator(x: f32, y: f32, width: f32) -> RuleRect {
    RuleRect {
        rect: [x, y + NOTE_SEPARATOR / 2.0, (width / 3.0).min(192.0), 0.75],
        color: Rgba::BLACK,
    }
}

/// Lay a document out on pages: sections, headers and footers, footnotes
/// and endnotes.
pub fn lay_out_with(doc: &Document, layout: &mut LayoutFn) -> Layout {
    lay_out_body(doc, layout, None, None, None).0
}

/// Lays out the body's layout paragraphs at their widths all at once (on
/// several threads, say) instead of one by one.
pub(crate) type BatchFn<'a> =
    dyn FnMut(&[docs_layout::Paragraph], &[f32]) -> Vec<Arc<ParaLayout>> + 'a;

/// The body paragraphs as [`lay_out_body`] paginated them: each one's
/// column width and its layout before pagination.
pub(crate) struct Body {
    pub widths: Vec<f32>,
    pub shaped: Vec<Arc<ParaLayout>>,
    /// The layout paragraphs, blocks and sections paginated.
    pub lps: Arc<Vec<docs_layout::Paragraph>>,
    pub blocks: Vec<Block>,
    pub flows: Vec<SectionFlow>,
    /// The headers' and footers' `STYLEREF` fields (one per story).
    pub style_refs: Vec<docs_engine::StyleRefs>,
    /// Each paragraph's list marker, the note references, and what each
    /// page's header and footer show (to build and lay out only what an
    /// edit changes next time).
    pub markers: Vec<Option<String>>,
    pub refs: Vec<NoteRef>,
    pub furn_keys: Vec<FurnKey>,
    /// The width (twips) each table in `blocks` was built for.
    pub table_widths: Vec<u32>,
}

/// What a page's header and footer show and where: two pages with the
/// same key get the same ones.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct FurnKey {
    stories: (Option<usize>, Option<usize>),
    label: String,
    number: u32,
    total: usize,
    section_pages: usize,
    setup: PageSetup,
    dists: (u32, u32),
    refs: (Vec<String>, Vec<String>),
}

/// The last layout of the body, to take what an edit didn't change from:
/// its pages, and its body (whose paragraphs, layouts and markers a known
/// edit takes over).
pub(crate) struct Prev<'a> {
    pub laid: &'a Layout,
    pub body: &'a mut Body,
    /// The edit since, when it's known: `.1` paragraphs from `.0` on
    /// became `.2`.
    pub splice: Option<(usize, usize, usize)>,
}

/// Where a new pagination of the body can take pages from the last one
/// (see [`docs_layout::Reuse`]): the blocks at the end that are the old
/// ones (their paragraphs shifted), laid out the same.
#[allow(clippy::too_many_arguments)]
fn reuse_of<'a>(
    old_paged: &'a PagedDocument,
    old: &Body,
    known: Option<(usize, usize)>,
    lps: &[docs_layout::Paragraph],
    layouts: &[Arc<ParaLayout>],
    blocks: &[Block],
    flows: &[SectionFlow],
) -> Option<docs_layout::Reuse<'a>> {
    if flows.len() != old.flows.len() {
        return None;
    }
    // The paragraphs the same at the end: known (built incrementally:
    // `.0` on, of `.1` before), or found by comparing.
    let (first_same, old_len) = match known {
        Some(k) => k,
        None => {
            if old.lps.len() != old.shaped.len() {
                return None;
            }
            let same = lps
                .iter()
                .zip(layouts)
                .rev()
                .zip(old.lps.iter().zip(&old.shaped).rev())
                .take_while(|((p, l), (q, m))| Arc::ptr_eq(l, m) && p == q)
                .count();
            (lps.len() - same, old.lps.len())
        }
    };
    let d = lps.len() as isize - old_len as isize;
    let shifted = |i: usize| i as isize - d;
    let para_ok = |i: usize, j: usize| i >= first_same && shifted(i) == j as isize;
    let cells_ok = |a: &docs_layout::TableBlock, b: &docs_layout::TableBlock| {
        a.x == b.x
            && a.columns == b.columns
            && a.padding == b.padding
            && a.rows.len() == b.rows.len()
            && a.rows.iter().zip(&b.rows).all(|(r, q)| {
                r.min_height == q.min_height
                    && (r.exact, r.header, r.cant_split, r.keep_next)
                        == (q.exact, q.header, q.cant_split, q.keep_next)
                    && r.cells.len() == q.cells.len()
                    && r.cells.iter().zip(&q.cells).all(|(c, e)| {
                        (c.col, c.span, c.merge, c.borders, c.fill)
                            == (e.col, e.span, e.merge, e.borders, e.fill)
                            && c.paras.len() == e.paras.len()
                            && (c.paras.is_empty() || para_ok(c.paras.start, e.paras.start))
                    })
            })
    };
    // Blocks the same at the end.
    let db = blocks.len() as isize - old.blocks.len() as isize;
    let same_blocks = blocks
        .iter()
        .rev()
        .zip(old.blocks.iter().rev())
        .take_while(|(a, b)| match (a, b) {
            (Block::Para(i), Block::Para(j)) => para_ok(*i, *j),
            (Block::Table(a), Block::Table(b)) => cells_ok(a, b),
            (Block::Gap(a), Block::Gap(b)) => a == b,
            _ => false,
        })
        .count();
    let from_block = blocks.len() - same_blocks;
    // The same sections, those starting in the same blocks moved along.
    for (a, b) in flows.iter().zip(&old.flows) {
        let moved = if a.first_block >= from_block && a.first_block > 0 {
            a.first_block as isize - db == b.first_block as isize
        } else {
            a.first_block == b.first_block
        };
        if !moved
            || a.setup != b.setup
            || a.first_setup != b.first_setup
            || a.even_setup != b.even_setup
            || a.start != b.start
            || a.restart != b.restart
            || a.columns != b.columns
            || a.sep != b.sep
        {
            return None;
        }
    }
    let renumber = flows.iter().all(|f| {
        f.even_setup.is_none() && !matches!(f.start, SectionBreak::EvenPage | SectionBreak::OddPage)
    });
    Some(docs_layout::Reuse {
        old: old_paged,
        from_block,
        block_shift: db,
        para_shift: d,
        renumber,
        from_para: first_same,
    })
}

/// [`lay_out_with`], and the body paragraphs' widths and layouts. With
/// `limit`, only about the first `limit` paragraphs are laid out (the
/// start of a long document, shown while the rest is laid out: the
/// others get an empty layout and no place on a page). `batch` lays out
/// the body's paragraphs (else `layout`, one at a time).
pub(crate) fn lay_out_body(
    doc: &Document,
    layout: &mut LayoutFn,
    limit: Option<usize>,
    batch: Option<&mut BatchFn>,
    prev: Option<Prev>,
) -> (Layout, Body) {
    use docs_layout::perf::span;
    let _all = span("docs lay_out_with");
    let t = span("docs sections+refs");
    let mut sections = doc.sections();
    let mut refs = doc.note_refs();
    let cut = limit.map_or(doc.len(), |l| l.min(doc.len()));
    if cut < doc.len() {
        // Notes of the paragraphs laid out; no endnotes yet.
        refs.retain(|r| r.pos.para < cut && r.kind == NoteKind::Footnote);
        sections.retain(|s| s.paras.start < cut || s.paras.start == 0);
    }
    drop(t);
    let mut prev = prev;
    // Footnotes numbered per page (renumbered from the layout below).
    let per_page = (0..sections.len()).any(|si| {
        refs.iter()
            .any(|r| r.kind == NoteKind::Footnote && r.section == si)
            && doc.note_props_in(&sections, NoteKind::Footnote, si).restart == NoteRestart::EachPage
    });
    // After a known edit: what it didn't touch is built and laid out as
    // before (see build::build_body).
    let incremental = prev.as_mut().and_then(|p| {
        let (start, removed, inserted) = p.splice?;
        let b = &mut *p.body;
        let old_len = b.lps.len();
        let ok = cut == doc.len()
            && !per_page
            && doc.field_codes().is_none()
            && doc.merge_view().is_none()
            && old_len + inserted == doc.len() + removed
            && b.shaped.len() == old_len
            && b.markers.len() == old_len
            && b.widths.len() == old_len
            && start + removed <= old_len;
        if !ok {
            return None;
        }
        let dirty = changed_note_paras(&b.refs, &refs, (start, removed, inserted));
        let lps = match Arc::try_unwrap(std::mem::take(&mut b.lps)) {
            Ok(v) => v,
            Err(shared) => shared.as_ref().clone(),
        };
        let prior = build::Prior {
            lps,
            markers: std::mem::take(&mut b.markers),
            widths: std::mem::take(&mut b.widths),
            start,
            removed,
            inserted,
            dirty,
            tables: b
                .blocks
                .iter()
                .filter_map(|blk| match blk {
                    Block::Table(t) => Some(t.clone()),
                    _ => None,
                })
                .zip(b.table_widths.iter().copied())
                .collect(),
            old: doc
                .last_splice()
                .filter(|sp| {
                    (sp.start, sp.removed, sp.inserted) == (start, removed, inserted)
                        && sp.old.len() == removed
                })
                .map(|sp| sp.old.clone()),
        };
        Some((
            prior,
            std::mem::take(&mut b.shaped),
            (start, removed, inserted),
            old_len,
        ))
    });
    let mut notes_cache: NoteCache = HashMap::new();
    let t = span("docs build body");
    let (prior, old_shaped, splice, old_len) = match incremental {
        Some((p, s, sp, n)) => (Some(p), Some(s), Some(sp), Some(n)),
        None => (None, None, None, None),
    };
    let built = body_with_notes(doc, &refs, &sections, &mut notes_cache, layout, cut, prior);
    let build::Built {
        mut lps,
        mut widths,
        mut blocks,
        markers,
        fresh,
        table_widths,
    } = built;
    drop(t);
    let t = span("docs shape body");
    let mut layouts: Vec<Arc<ParaLayout>> = match (old_shaped, splice) {
        // The old layouts where the paragraphs weren't built again.
        (Some(mut old), Some((start, removed, inserted))) => {
            let tail = old.split_off(start + removed);
            old.truncate(start);
            for k in start..start + inserted {
                old.push(layout(&lps[k], widths[k]));
            }
            old.extend(tail);
            for (k, _) in fresh.iter().enumerate().filter(|(_, f)| **f) {
                if !(start..start + inserted).contains(&k) {
                    old[k] = layout(&lps[k], widths[k]);
                }
            }
            old
        }
        _ => match batch {
            Some(batch) => batch(&lps, &widths),
            None => lps
                .iter()
                .zip(&widths)
                .map(|(p, &w)| layout(p, w))
                .collect(),
        },
    };
    // From here on every paragraph is as before (shifted).
    let known_same = old_len.map(|n| {
        let last_fresh = fresh.iter().rposition(|f| *f).map_or(0, |k| k + 1);
        let (start, _, inserted) = splice.unwrap_or_default();
        (last_fresh.max(start + inserted).min(lps.len()), n)
    });
    drop(t);
    // Paragraphs not laid out yet: an empty layout each (no lines placed).
    if layouts.len() < doc.len() {
        let empty = layout(&docs_layout::Paragraph::new(String::new()), 100.0);
        layouts.resize(doc.len(), empty);
        widths.resize(doc.len(), 100.0);
    }

    // Header/footer heights (with placeholder numbers) push the text in.
    let t = span("docs sections setup");
    let mut heights: HashMap<(usize, u32), f32> = HashMap::new();
    let mut height_of = |story: Option<usize>, width: f32, layout: &mut LayoutFn| -> f32 {
        let Some(i) = story else { return 0.0 };
        *heights
            .entry((i, width.to_bits()))
            .or_insert_with(|| story_layout(doc, i, width, ("1", "1", "1"), &[], layout).height)
    };
    let mut flows = Vec::with_capacity(sections.len());
    let mut stories = Vec::with_capacity(sections.len());
    for (si, s) in sections.iter().enumerate() {
        let base = build::geometry_setup(&s.props.page);
        let width = base.content_width();
        let st = section_stories(doc, &sections, si);
        let hd = twips(s.props.header_dist);
        let fd = twips(s.props.footer_dist);
        let mut with = |(h, f): (Option<usize>, Option<usize>), layout: &mut LayoutFn| {
            let hh = height_of(h, width, layout);
            let fh = height_of(f, width, layout);
            let mut setup = base;
            if hh > 0.0 {
                setup.margin_top = setup.margin_top.max(hd + hh);
            }
            if fh > 0.0 {
                setup.margin_bottom = setup.margin_bottom.max(fd + fh);
            }
            setup
        };
        let setup = with(st.default, layout);
        let first = with(st.first, layout);
        let even = with(st.even, layout);
        let first_block = blocks
            .iter()
            .position(|b| block_start(b) >= s.paras.start)
            .unwrap_or(blocks.len());
        flows.push(SectionFlow {
            first_block: if si == 0 { 0 } else { first_block },
            setup,
            first_setup: (first != setup).then_some(first),
            even_setup: (even != setup).then_some(even),
            start: match s.props.start {
                SectionStart::NextPage => SectionBreak::NextPage,
                SectionStart::Continuous => SectionBreak::Continuous,
                SectionStart::NextColumn => SectionBreak::NextColumn,
                SectionStart::EvenPage => SectionBreak::EvenPage,
                SectionStart::OddPage => SectionBreak::OddPage,
            },
            restart: s.props.page_start,
            columns: section_columns(s),
            sep: s.props.columns.sep,
        });
        stories.push(st);
    }
    drop(t);
    let t = span("docs endnotes");
    let (mut gapped, mut gap_flows) = with_endnotes(
        doc,
        &refs,
        &sections,
        blocks,
        &flows,
        &mut notes_cache,
        layout,
    );
    drop(t);
    let t = span("docs paginate");
    // After an edit: the pages after it that come out the same are taken
    // from the last layout (see docs_layout::Reuse).
    let reuse = prev.as_ref().and_then(|p| {
        let _t = span("docs compare with last layout");
        reuse_of(
            &p.laid.paged,
            p.body,
            known_same,
            &lps,
            &layouts,
            &gapped,
            &gap_flows,
        )
    });
    let mut paged =
        PagedDocument::paginate_reusing(&lps, layouts.clone(), &gapped, &gap_flows, reuse.as_ref());
    drop(t);
    // Footnotes numbered per page: renumber where they landed and lay out
    // again (the numbers' widths can move text).
    if per_page {
        let renumbered = renumber_per_page(doc, &sections, &refs, &paged);
        if renumbered != refs {
            refs = renumbered;
            let b = body_with_notes(doc, &refs, &sections, &mut notes_cache, layout, cut, None);
            (lps, widths, blocks) = (b.lps, b.widths, b.blocks);
            layouts = lps
                .iter()
                .zip(&widths)
                .map(|(p, &w)| layout(p, w))
                .collect();
            if layouts.len() < doc.len() {
                let empty = layout(&docs_layout::Paragraph::new(String::new()), 100.0);
                layouts.resize(doc.len(), empty);
                widths.resize(doc.len(), 100.0);
            }
            (gapped, gap_flows) = with_endnotes(
                doc,
                &refs,
                &sections,
                blocks,
                &flows,
                &mut notes_cache,
                layout,
            );
            paged = PagedDocument::paginate_sections(&lps, layouts.clone(), &gapped, &gap_flows);
        }
    }
    // Pages as in the last layout: before the first page the edit
    // changed (not moved), and from where pages were taken from it (see
    // docs_layout::Reuse; paragraphs moved by `shift`).
    let last = prev.as_ref().map(|p| p.laid);
    let shift = old_len.map_or(0, |n| lps.len() as isize - n as isize);
    let taken = paged.reused().filter(|_| last.is_some());
    let moved_from = reuse.as_ref().map_or(0, |r| r.from_para);
    let unmoved = match (last, splice) {
        (Some(laid), Some((start, _, _))) => {
            // Not past the page the edit starts on (text changes there
            // needn't move lines), nor past where the pages differ.
            let first = fresh.iter().position(|f| *f).unwrap_or(start).min(start);
            let edit_page = laid
                .paged
                .caret(first.saturating_sub(1), 0)
                .map_or(0, |c| c.page);
            let differ = paged
                .pages
                .iter()
                .zip(&laid.paged.pages)
                .position(|(a, b)| a != b)
                .unwrap_or(paged.pages.len().min(laid.paged.pages.len()));
            edit_page.min(differ)
        }
        _ => 0,
    };
    // A page's counterpart in the last layout, and whether paragraphs
    // moved since.
    let old_of = |pi: usize| -> Option<(usize, bool)> {
        if pi < unmoved {
            return Some((pi, false));
        }
        let (at, j) = taken?;
        (pi >= at).then(|| (pi - at + j, true))
    };
    if docs_layout::perf::enabled() && prev.is_some() {
        eprintln!(
            "[perf] docs reuse: known {:?}, from block {:?} of {}, pages taken {:?}, unmoved {unmoved}, of {}",
            known_same,
            reuse.as_ref().map(|r| r.from_block),
            gapped.len(),
            taken,
            paged.pages.len()
        );
    }
    let t = span("docs place notes");
    let mut notes = place_notes(
        doc,
        &refs,
        &sections,
        &mut paged,
        &mut notes_cache,
        layout,
        &|pi| old_of(pi).is_some(),
    );
    if let Some(laid) = last {
        // Their notes as they were.
        for (pi, out) in notes.iter_mut().enumerate() {
            if let Some((o, _)) = old_of(pi) {
                *out = laid
                    .furniture
                    .get(o)
                    .into_iter()
                    .flatten()
                    .filter(|p| p.layout.story == usize::MAX)
                    .cloned()
                    .collect();
            }
        }
    }
    drop(t);
    let t = span("docs headers+footers");
    let lps = Arc::new(lps);
    let strikes = if lps.iter().any(|p| !p.strikes.is_empty()) {
        lps.clone()
    } else {
        Arc::default()
    };
    // Each header's and footer's STYLEREF fields (once: the long body is
    // searched for each).
    let style_refs: Vec<docs_engine::StyleRefs> = doc
        .stories()
        .iter()
        .map(|st| docs_engine::StyleRefs::new(doc, st.doc.paragraphs()))
        .collect();

    // Each page's header and footer, numbers filled in (laying out the
    // start of a document: the number of pages it should have).
    let total = if cut < doc.len() {
        estimate_pages(doc, cut, paged.pages.len())
    } else {
        paged.pages.len()
    };
    let total_text = total.to_string();
    let mut in_section: HashMap<usize, usize> = HashMap::new();
    for page in &paged.pages {
        *in_section.entry(page.section).or_default() += 1;
    }
    type Key = (usize, String, String, u32, Vec<String>);
    let mut cache: HashMap<Key, Arc<StoryLayout>> = HashMap::new();
    // The body paragraphs on each page (for STYLEREF in headers and footers).
    let mut on_page = Vec::with_capacity(paged.pages.len());
    let mut last_para = 0;
    for page in &paged.pages {
        let paras: Vec<usize> = page
            .lines
            .iter()
            .filter(|l| !l.repeat)
            .map(|l| l.para)
            .collect();
        let range = match (paras.iter().min(), paras.iter().max()) {
            (Some(&a), Some(&b)) => (a, b),
            _ => (last_para, last_para),
        };
        last_para = range.1;
        on_page.push(range);
    }
    let t2 = span("docs page labels");
    let labels = page_labels(doc, &sections, &paged);
    drop(t2);
    let old_keys = prev.as_ref().map(|p| &p.body.furn_keys);
    let mut furniture = Vec::with_capacity(paged.pages.len());
    let mut furn_keys = Vec::with_capacity(paged.pages.len());
    for (pi, page) in paged.pages.iter().enumerate() {
        let si = page.section.min(sections.len() - 1);
        let props = &sections[si].props;
        let first_of_section = pi == 0 || paged.pages[pi - 1].section != page.section;
        let st = &stories[si];
        let (h, f) = if first_of_section {
            st.first
        } else if page.number.is_multiple_of(2) {
            st.even
        } else {
            st.default
        };
        let section_pages = in_section.get(&page.section).copied().unwrap_or(1);
        let refs_of = |story: Option<usize>| {
            story
                .and_then(|i| style_refs.get(i))
                .map(|r| r.results(doc, on_page[pi]))
                .unwrap_or_default()
        };
        let key = FurnKey {
            stories: (h, f),
            label: labels[pi].clone(),
            number: page.number,
            total,
            section_pages,
            setup: page.setup,
            dists: (props.header_dist, props.footer_dist),
            refs: (refs_of(h), refs_of(f)),
        };
        // The same header and footer as its counterpart in the last
        // layout: those (and its notes).
        if let (Some(laid), Some(keys), Some((o, _))) = (last, old_keys, old_of(pi))
            && keys.get(o) == Some(&key)
            && let Some(old) = laid.furniture.get(o)
        {
            let mut placed: Vec<Placed> = old
                .iter()
                .filter(|p| p.layout.story != usize::MAX)
                .cloned()
                .collect();
            placed.extend(notes.get(pi).into_iter().flatten().cloned());
            furniture.push(placed);
            furn_keys.push(key);
            continue;
        }
        let number = key.label.clone();
        let setup = page.setup;
        let width = setup.content_width();
        let mut placed = Vec::new();
        for (kind, story, refs) in [
            (HfKind::Header, h, &key.refs.0),
            (HfKind::Footer, f, &key.refs.1),
        ] {
            let Some(i) = story else { continue };
            let section_pages = section_pages.to_string();
            let l = cache
                .entry((
                    i,
                    number.clone(),
                    section_pages.clone(),
                    width.to_bits(),
                    refs.clone(),
                ))
                .or_insert_with(|| {
                    Arc::new(story_layout(
                        doc,
                        i,
                        width,
                        (&number, &total_text, &section_pages),
                        refs,
                        layout,
                    ))
                })
                .clone();
            let y = match kind {
                HfKind::Header => twips(props.header_dist),
                HfKind::Footer => setup.height - twips(props.footer_dist) - l.height,
            };
            placed.push(Placed {
                kind,
                layout: l,
                x: setup.margin_left,
                y,
                note: None,
            });
        }
        placed.extend(notes.get(pi).into_iter().flatten().cloned());
        furniture.push(placed);
        furn_keys.push(key);
    }
    drop(t);
    let t = span("docs place floats");
    // Floating objects of pages as before, with the same header and
    // footer (header objects follow the page number).
    let same_page = |pi: usize| -> Option<(usize, bool)> {
        let (o, moved) = old_of(pi)?;
        let keys = old_keys?;
        (keys.get(o) == furn_keys.get(pi)).then_some((o, moved))
    };
    let mut floats = place_floats(doc, &paged, &furniture, layout, &|pi| {
        same_page(pi).is_some_and(|(o, _)| last.is_some_and(|l| o < l.floats.len()))
    });
    if let Some(laid) = last {
        for (pi, items) in floats.iter_mut().enumerate() {
            let Some((o, moved)) = same_page(pi) else {
                continue;
            };
            let Some(old) = laid.floats.get(o) else {
                continue;
            };
            *items = old.clone();
            if moved {
                // (Paragraphs before the edit didn't move.)
                for it in items.iter_mut().filter(|it| {
                    it.story.is_none() && it.para as isize + shift >= moved_from as isize
                }) {
                    it.para = (it.para as isize + shift).max(0) as usize;
                }
            }
        }
    }
    drop(t);
    (
        Layout {
            paged,
            furniture,
            floats,
            strikes,
        },
        Body {
            widths,
            shaped: layouts,
            lps,
            blocks: gapped,
            flows: gap_flows,
            style_refs,
            markers,
            refs,
            furn_keys,
            table_widths,
        },
    )
}

/// Paragraphs (new numbering) whose footnote and endnote references
/// changed besides those an edit (`removed` paragraphs from `start` on
/// became `inserted`) touched: their numbers, say.
fn changed_note_paras(
    old: &[NoteRef],
    new: &[NoteRef],
    (start, removed, inserted): (usize, usize, usize),
) -> Vec<usize> {
    use std::collections::HashSet;
    type Mark<'a> = (usize, usize, NoteKind, &'a str, &'a str);
    let moved = |para: usize| {
        if para < start {
            Some(para)
        } else if para >= start + removed {
            Some(para - removed + inserted)
        } else {
            None
        }
    };
    let before: HashSet<Mark> = old
        .iter()
        .filter_map(|r| {
            let para = moved(r.pos.para)?;
            Some((para, r.pos.offset, r.kind, r.id.as_str(), r.label.as_str()))
        })
        .collect();
    let after: HashSet<Mark> = new
        .iter()
        .map(|r| {
            (
                r.pos.para,
                r.pos.offset,
                r.kind,
                r.id.as_str(),
                r.label.as_str(),
            )
        })
        .collect();
    let mut out: Vec<usize> = before.symmetric_difference(&after).map(|m| m.0).collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// About how many pages `doc` has when its first `cut` paragraphs take
/// `pages` pages (by their share of its text).
pub fn estimate_pages(doc: &Document, cut: usize, pages: usize) -> usize {
    let paras = doc.paragraphs();
    let len = |ps: &[docs_engine::Paragraph]| ps.iter().map(|p| p.text.len() + 40).sum::<usize>();
    let laid = len(&paras[..cut.min(paras.len())]).max(1);
    let all = len(paras);
    (pages * all).div_ceil(laid).max(pages)
}

/// Every page's number as its section shows it (format, and the chapter
/// number before it when the section includes one).
pub(crate) fn page_labels(
    doc: &Document,
    sections: &[docs_engine::Section],
    paged: &PagedDocument,
) -> Vec<String> {
    let mut chapters: HashMap<u8, Vec<Option<String>>> = HashMap::new();
    let mut last_para = 0;
    paged
        .pages
        .iter()
        .map(|page| {
            let Some(s) = sections.get(page.section).or(sections.last()) else {
                return page.number.to_string();
            };
            if let Some(l) = page.lines.iter().find(|l| !l.repeat) {
                last_para = l.para;
            }
            let chapter = s.props.chapter.and_then(|(level, _)| {
                chapters
                    .entry(level)
                    .or_insert_with(|| doc.chapter_numbers(level))
                    .get(last_para)
                    .cloned()
                    .flatten()
            });
            s.props.page_label(page.number, chapter.as_deref())
        })
        .collect()
}

/// A floating object's item on a page, its text box laid out.
fn float_item(
    doc: &Document,
    (story, para, index): (Option<usize>, usize, usize),
    rect: [f32; 4],
    painter: &floats::Painter,
    layout: &mut LayoutFn,
) -> Option<FloatItem> {
    let owner = match story {
        Some(s) => &doc.stories().get(s)?.doc,
        None => doc,
    };
    let f = owner.float_at(para, index)?;
    let mut lay_box = |tb: &docs_engine::float::TextBox,
                       [x, y, width, room]: [f32; 4],
                       group_box: Option<usize>| {
        let mut sl = lay_story(
            &tb.doc,
            tb.doc.paragraphs().to_vec(),
            width,
            &Marks::default(),
            layout,
            usize::MAX,
            None,
        );
        sl.text_box = Some((story, para, index));
        sl.group_box = group_box;
        let dy = match tb.v_align {
            docs_engine::float::FloatAlign::Center => ((room - sl.height) / 2.0).max(0.0),
            docs_engine::float::FloatAlign::End => (room - sl.height).max(0.0),
            _ => 0.0,
        };
        (Arc::new(sl), x, y + dy)
    };
    let mut rect = rect;
    let text = f.text.as_ref().map(|tb| {
        let area = floats::text_area(f, rect);
        let laid = lay_box(tb, area, None);
        // "Resize shape to fit text": as tall as the text and margins
        // (while it's typed in, too; leaving the box stores the size).
        let need = laid.0.height + (rect[3] - area[3]);
        if tb.auto_fit && (need - rect[3]).abs() > 0.5 {
            rect[3] = need.max(1.0);
            return lay_box(tb, floats::text_area(f, rect), None);
        }
        laid
    });
    let mut boxes = Vec::new();
    if !f.boxes.is_empty() {
        for (k, area) in floats::group_text_areas(f, rect).into_iter().enumerate() {
            let (Some(a), Some(b)) = (area, f.boxes.get(k)) else {
                continue;
            };
            let (l, x, y) = lay_box(&b.text, a.area, Some(k));
            boxes.push(floats::BoxText {
                which: k,
                layout: l,
                x,
                y,
                rect: a.rect,
                turn: a.turn,
            });
        }
    }
    let mut chart =
        crate::chart::chart_draw(doc, f, (story, para, index), rect, painter, layout).map(Arc::new);
    if let Some(sa) = crate::smartart::draw(doc, f, (story, para, index), rect, painter, layout) {
        chart = Some(Arc::new(crate::chart::ChartDraw {
            prims: sa.prims,
            labels: Vec::new(),
        }));
        boxes.extend(sa.texts);
    }
    Some(FloatItem {
        para,
        index,
        story,
        rect,
        z: f.at.z,
        behind: f.at.behind,
        text,
        chart,
        boxes,
    })
}

/// Every page's floating objects: the body's where the layout put them,
/// headers' and footers' from where those landed.
fn place_floats(
    doc: &Document,
    paged: &PagedDocument,
    furniture: &[Vec<Placed>],
    layout: &mut LayoutFn,
    skip: &dyn Fn(usize) -> bool,
) -> Vec<Vec<FloatItem>> {
    let mut out = Vec::with_capacity(paged.pages.len());
    let painter = floats::Painter::new(doc);
    for (pi, page) in paged.pages.iter().enumerate() {
        // The caller has these.
        if skip(pi) {
            out.push(Vec::new());
            continue;
        }
        let mut items = Vec::new();
        for pf in &page.floats {
            if let Some(it) = float_item(doc, (None, pf.para, pf.index), pf.rect, &painter, layout)
            {
                items.push(it);
            }
        }
        // Inline SmartArt: drawn into its box in the line.
        for o in paged.objects(pi) {
            if o.kind == ObjectKind::Drawing
                && let Some(it) =
                    float_item(doc, (None, o.para, o.index), o.whole, &painter, layout)
            {
                items.push(it);
            }
        }
        for placed in furniture.get(pi).into_iter().flatten() {
            if placed.note.is_some() || placed.layout.note.is_some() {
                continue;
            }
            let story = placed.layout.story;
            let Some(st) = doc.stories().get(story) else {
                continue;
            };
            for o in placed.layout.paged.objects(0) {
                if o.kind != ObjectKind::Drawing {
                    continue;
                }
                let [x, y, w, h] = o.whole;
                let rect = [x + placed.x, y + placed.y, w, h];
                if let Some(it) =
                    float_item(doc, (Some(story), o.para, o.index), rect, &painter, layout)
                {
                    items.push(it);
                }
            }
            for (para, p) in st.doc.paragraphs().iter().enumerate() {
                for (index, o) in p.objects() {
                    let docs_engine::Object::Float(f) = o else {
                        continue;
                    };
                    if f.inline {
                        continue;
                    }
                    let line = placed.layout.paged.caret(para, index);
                    let top = placed
                        .layout
                        .paged
                        .caret(para, 0)
                        .map_or(placed.y, |c| placed.y + c.top);
                    let at = docs_layout::AnchorAt {
                        para_top: top,
                        line_top: line.map_or(top, |c| placed.y + c.top),
                        char_x: line.map_or(placed.x, |c| placed.x + c.x),
                        column: None,
                    };
                    let rect = floats::place(f, index, &page.setup, page.number, at);
                    if let Some(it) =
                        float_item(doc, (Some(story), para, index), rect, &painter, layout)
                    {
                        items.push(it);
                    }
                }
            }
        }
        // Behind the text first, then by z-order (stable: document order).
        items.sort_by_key(|it| (!it.behind, it.z));
        out.push(items);
    }
    out
}

/// A note's layout, or the part `from`..`to` of it.
fn note_part(l: Arc<StoryLayout>, from: f32, to: f32) -> Arc<StoryLayout> {
    if from <= 0.01 && to >= l.height - 0.01 {
        return l;
    }
    Arc::new(StoryLayout {
        story: l.story,
        note: l.note,
        text_box: l.text_box,
        group_box: l.group_box,
        paged: l.paged.slice(0, from, to),
        paras: l.paras.clone(),
        height: to - from,
        strikes: l.strikes.clone(),
    })
}

/// Place footnotes at the bottom of their pages (a long one continued
/// under Word's full-width continuation separator, with the document's
/// continuation notice under the part that goes on) and endnotes where
/// the flow left room for them. Returns each page's notes.
fn place_notes(
    doc: &Document,
    refs: &[NoteRef],
    sections: &[Section],
    paged: &mut PagedDocument,
    cache: &mut NoteCache,
    layout: &mut LayoutFn,
    skip: &dyn Fn(usize) -> bool,
) -> Vec<Vec<Placed>> {
    let mut out: Vec<Vec<Placed>> = vec![Vec::new(); paged.pages.len()];
    let find = |para: usize, index: usize| {
        refs.iter().find(|r| {
            r.kind == NoteKind::Footnote && r.pos == docs_engine::Position::new(para, index)
        })
    };
    for (pi, page) in paged.pages.iter_mut().enumerate() {
        // Pages as in the last layout have theirs placed already.
        if skip(pi) {
            continue;
        }
        // Each column's notes under it (the whole text width without
        // columns).
        let columns = std::mem::take(&mut page.note_columns);
        for nc in &columns {
            let mut y = nc.bottom - nc.height;
            let rule = if nc.continued {
                // Word's continuation separator: the text column's width.
                RuleRect {
                    rect: [nc.x, y + NOTE_SEPARATOR / 2.0, nc.width, 0.75],
                    color: Rgba::BLACK,
                }
            } else {
                separator(nc.x, y, nc.width)
            };
            page.rules.push(rule);
            y += NOTE_SEPARATOR;
            for part in &nc.parts {
                let Some(r) = find(part.para, part.index) else {
                    continue;
                };
                let Some(i) = doc.note_index(r.kind, &r.id) else {
                    continue;
                };
                // The note as wide as its column.
                let width = nc.width;
                let l = cached_note(cache, doc, i, width, &r.label, layout);
                let l = note_part(l, part.from, part.to);
                let h = l.height;
                out[pi].push(Placed {
                    kind: HfKind::Footer,
                    layout: l,
                    x: nc.x,
                    y,
                    note: Some((r.kind, r.id.clone())),
                });
                y += h;
                if part.more
                    && let Some(n) = notice_layout(doc, r.kind, width, cache, layout)
                {
                    let h = n.height;
                    out[pi].push(Placed {
                        kind: HfKind::Footer,
                        layout: n,
                        x: nc.x,
                        y,
                        note: None,
                    });
                    y += h;
                }
            }
        }
        page.note_columns = columns;
        for g in &page.gaps {
            if g.id == SEPARATOR_GAP {
                let width = section_width(sections, page.section);
                page.rules.push(separator(g.x, g.y, width));
                continue;
            }
            let Some(r) = refs.get(g.id) else { continue };
            let Some(i) = doc.note_index(r.kind, &r.id) else {
                continue;
            };
            let l = cached_note(
                cache,
                doc,
                i,
                section_width(sections, r.section),
                &r.label,
                layout,
            );
            out[pi].push(Placed {
                kind: HfKind::Footer,
                layout: note_part(l, g.from, g.to),
                x: g.x,
                y: g.y,
                note: Some((r.kind, r.id.clone())),
            });
        }
    }
    out
}

/// The first paragraph a block holds.
fn block_start(b: &Block) -> usize {
    match b {
        Block::Para(i) => *i,
        Block::Gap(_) => usize::MAX,
        Block::Table(t) => t
            .rows
            .first()
            .and_then(|r| r.cells.first())
            .map_or(usize::MAX, |c| c.paras.start),
    }
}
