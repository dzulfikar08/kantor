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
//!   room. Endnotes follow the end of the document. A note doesn't break
//!   across pages. Numbers restart per section or per page as the
//!   document's footnote settings say.

use std::collections::HashMap;
use std::sync::Arc;

use docs_engine::{
    Document, HfKind, HfType, NoteKind, NoteRef, NoteRestart, Section, SectionStart,
    with_page_numbers,
};
use docs_layout::{
    Block, NOTE_SEPARATOR, Page, PageSetup, PagedDocument, ParaLayout, Rgba, RuleRect,
    SectionBreak, SectionFlow,
};

use crate::build::{self, Marks};

/// Lays out one paragraph broken at a width (the Docs view passes its
/// cache, export the engine).
pub type LayoutFn<'a> = dyn FnMut(&docs_layout::Paragraph, f32) -> Arc<ParaLayout> + 'a;

/// A header, footer or note laid out for some page.
pub struct StoryLayout {
    /// Index into [`Document::stories`] (headers and footers).
    pub story: usize,
    /// Index into [`Document::notes`] (footnotes and endnotes).
    pub note: Option<usize>,
    /// Laid out on one tall page whose content area starts at 0, 0.
    pub paged: PagedDocument,
    /// The story's paragraphs as laid out (page numbers filled in).
    pub paras: Vec<docs_engine::Paragraph>,
    /// Height of its content, px.
    pub height: f32,
}

impl StoryLayout {
    pub fn text(&self, para: usize) -> &str {
        self.paras.get(para).map_or("", |p| p.text.as_str())
    }

    /// The small document laid out (for its pictures).
    pub fn doc<'a>(&self, doc: &'a Document) -> &'a Document {
        match self.note {
            Some(i) => &doc.notes()[i].doc,
            None => &doc.stories()[self.story].doc,
        }
    }

    /// A key telling stories apart (picture caches).
    pub fn key(&self) -> usize {
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

/// A story laid out at `width` px with `page` / `pages` as its numbers.
fn story_layout(
    doc: &Document,
    index: usize,
    width: f32,
    (page, pages): (&str, &str),
    layout: &mut LayoutFn,
) -> StoryLayout {
    let sd = &doc.stories()[index].doc;
    let paras =
        with_page_numbers(sd.paragraphs(), page, pages).unwrap_or_else(|| sd.paragraphs().to_vec());
    lay_story(sd, paras, width, &Marks::default(), layout, index, None)
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
    let (lps, widths, blocks) = build::build_with(sd, &paras, &|_| width_tw, marks);
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
    StoryLayout {
        story,
        note,
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
) -> (Vec<docs_layout::Paragraph>, Vec<f32>, Vec<Block>) {
    let marks = Marks {
        refs: refs
            .iter()
            .map(|r| ((r.kind, r.id.clone()), r.label.clone()))
            .collect(),
        own: None,
    };
    let (mut lps, widths, blocks) = build::build_marked(doc, &marks);
    for r in refs.iter().filter(|r| r.kind == NoteKind::Footnote) {
        let Some(i) = doc.note_index(r.kind, &r.id) else {
            continue;
        };
        let width = section_width(sections, r.section);
        let h = cached_note(cache, doc, i, width, &r.label, layout).height;
        if let Some(lp) = lps.get_mut(r.pos.para) {
            lp.notes.push((r.pos.offset, h));
        }
    }
    (lps, widths, blocks)
}

/// A section's text width, px.
fn section_width(sections: &[Section], si: usize) -> f32 {
    sections
        .get(si)
        .map_or(PageSetup::a4().content_width(), |s| {
            build::geometry_setup(&s.props.page).content_width()
        })
}

/// Footnote numbers restarted on each page (sections set to "each page"),
/// from where the references landed.
fn renumber_per_page(doc: &Document, refs: &[NoteRef], paged: &PagedDocument) -> Vec<NoteRef> {
    let mut out = refs.to_vec();
    let mut count: HashMap<usize, u32> = HashMap::new();
    for r in &mut out {
        if r.kind != NoteKind::Footnote || r.label.is_empty() {
            continue;
        }
        let props = doc.note_props(r.kind, r.section);
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
    let sections = doc.sections();
    let mut refs = doc.note_refs();
    let mut notes_cache: NoteCache = HashMap::new();
    let (mut lps, mut widths, mut blocks) =
        body_with_notes(doc, &refs, &sections, &mut notes_cache, layout);
    let mut layouts: Vec<Arc<ParaLayout>> = lps
        .iter()
        .zip(&widths)
        .map(|(p, &w)| layout(p, w))
        .collect();

    // Header/footer heights (with placeholder numbers) push the text in.
    let mut heights: HashMap<(usize, u32), f32> = HashMap::new();
    let mut height_of = |story: Option<usize>, width: f32, layout: &mut LayoutFn| -> f32 {
        let Some(i) = story else { return 0.0 };
        *heights
            .entry((i, width.to_bits()))
            .or_insert_with(|| story_layout(doc, i, width, ("1", "1"), layout).height)
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
                SectionStart::EvenPage => SectionBreak::EvenPage,
                SectionStart::OddPage => SectionBreak::OddPage,
            },
            restart: s.props.page_start,
        });
        stories.push(st);
    }
    let mut paged = PagedDocument::paginate_sections(&lps, layouts.clone(), &blocks, &flows);
    // Footnotes numbered per page: renumber where they landed and lay out
    // again (the numbers' widths can move text).
    let per_page = refs.iter().any(|r| {
        r.kind == NoteKind::Footnote
            && doc.note_props(r.kind, r.section).restart == NoteRestart::EachPage
    });
    if per_page {
        let renumbered = renumber_per_page(doc, &refs, &paged);
        if renumbered != refs {
            refs = renumbered;
            (lps, widths, blocks) =
                body_with_notes(doc, &refs, &sections, &mut notes_cache, layout);
            layouts = lps
                .iter()
                .zip(&widths)
                .map(|(p, &w)| layout(p, w))
                .collect();
            paged = PagedDocument::paginate_sections(&lps, layouts, &blocks, &flows);
        }
    }
    let notes = place_notes(doc, &refs, &sections, &mut paged, &mut notes_cache, layout);
    let strikes = if lps.iter().any(|p| !p.strikes.is_empty()) {
        Arc::new(lps)
    } else {
        Arc::default()
    };

    // Each page's header and footer, numbers filled in.
    let total = paged.pages.len().to_string();
    let mut cache: HashMap<(usize, String, u32), Arc<StoryLayout>> = HashMap::new();
    let mut furniture = Vec::with_capacity(paged.pages.len());
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
        let number = props.num_format.format(page.number);
        let setup = page.setup;
        let width = setup.content_width();
        let mut placed = Vec::new();
        for (kind, story) in [(HfKind::Header, h), (HfKind::Footer, f)] {
            let Some(i) = story else { continue };
            let l = cache
                .entry((i, number.clone(), width.to_bits()))
                .or_insert_with(|| Arc::new(story_layout(doc, i, width, (&number, &total), layout)))
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
    }
    Layout {
        paged,
        furniture,
        strikes,
    }
}

/// Place footnotes at the bottom of their pages and endnotes after the
/// text (on new pages when they don't fit). Returns each page's notes.
fn place_notes(
    doc: &Document,
    refs: &[NoteRef],
    sections: &[Section],
    paged: &mut PagedDocument,
    cache: &mut NoteCache,
    layout: &mut LayoutFn,
) -> Vec<Vec<Placed>> {
    let mut out: Vec<Vec<Placed>> = vec![Vec::new(); paged.pages.len()];
    let find = |para: usize, index: usize| {
        refs.iter().find(|r| {
            r.kind == NoteKind::Footnote && r.pos == docs_engine::Position::new(para, index)
        })
    };
    for (pi, page) in paged.pages.iter_mut().enumerate() {
        if page.notes.is_empty() {
            continue;
        }
        let s = page.setup;
        let mut y = s.height - s.margin_bottom - page.notes_height;
        page.rules
            .push(separator(s.margin_left, y, s.content_width()));
        y += NOTE_SEPARATOR;
        for &(para, index) in &page.notes {
            let Some(r) = find(para, index) else { continue };
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
            let h = l.height;
            out[pi].push(Placed {
                kind: HfKind::Footer,
                layout: l,
                x: s.margin_left,
                y,
                note: Some((r.kind, r.id.clone())),
            });
            y += h;
        }
    }
    // Endnotes: after the last line of the document.
    let ends: Vec<&NoteRef> = refs
        .iter()
        .filter(|r| r.kind == NoteKind::Endnote && doc.note(r.kind, &r.id).is_some())
        .collect();
    if ends.is_empty() || paged.pages.is_empty() {
        return out;
    }
    let last = paged.pages.len() - 1;
    let used = {
        let p = &paged.pages[last];
        p.lines
            .iter()
            .map(|l| l.top + l.height)
            .chain(p.rules.iter().map(|r| r.rect[1] + r.rect[3]))
            .fold(p.setup.margin_top, f32::max)
    };
    let mut page = last;
    let mut y = used + 12.0;
    let s = paged.pages[last].setup;
    let bottom = |p: &Page| p.setup.height - p.setup.margin_bottom - p.notes_height;
    if y + NOTE_SEPARATOR > bottom(&paged.pages[page]) {
        page = new_page_after(paged, &mut out);
        y = paged.pages[page].setup.margin_top;
    }
    paged.pages[page]
        .rules
        .push(separator(s.margin_left, y, s.content_width()));
    y += NOTE_SEPARATOR;
    let width = s.content_width();
    for r in ends {
        let Some(i) = doc.note_index(r.kind, &r.id) else {
            continue;
        };
        let l = cached_note(cache, doc, i, width, &r.label, layout);
        if y + l.height > bottom(&paged.pages[page]) && y > paged.pages[page].setup.margin_top + 0.5
        {
            page = new_page_after(paged, &mut out);
            y = paged.pages[page].setup.margin_top;
        }
        let h = l.height;
        out[page].push(Placed {
            kind: HfKind::Footer,
            layout: l,
            x: paged.pages[page].setup.margin_left,
            y,
            note: Some((r.kind, r.id.clone())),
        });
        y += h;
    }
    out
}

/// A blank page after the last one (same section and size): endnotes that
/// don't fit go on.
fn new_page_after(paged: &mut PagedDocument, out: &mut Vec<Vec<Placed>>) -> usize {
    let last = paged.pages.last().cloned().unwrap_or_default();
    paged.pages.push(Page {
        setup: last.setup,
        section: last.section,
        number: last.number + 1,
        ..Page::default()
    });
    out.push(Vec::new());
    paged.pages.len() - 1
}

/// The first paragraph a block holds.
fn block_start(b: &Block) -> usize {
    match b {
        Block::Para(i) => *i,
        Block::Table(t) => t
            .rows
            .first()
            .and_then(|r| r.cells.first())
            .map_or(usize::MAX, |c| c.paras.start),
    }
}
