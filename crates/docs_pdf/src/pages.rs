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
    Block, NOTE_SEPARATOR, PageSetup, PagedDocument, ParaLayout, Rgba, RuleRect, SectionBreak,
    SectionFlow,
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
                .and_then(|f| f.text.as_ref())
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
            return (3 << 40) + (story.map_or(0, |s| s + 1) << 32) + (para << 12) + index;
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
    layout: &mut LayoutFn,
) -> StoryLayout {
    let sd = &doc.stories()[index].doc;
    let marks = Marks {
        pages: Some([page.to_owned(), pages.to_owned(), section_pages.to_owned()]),
        ..Marks::default()
    };
    lay_story(
        sd,
        sd.paragraphs().to_vec(),
        width,
        &marks,
        layout,
        index,
        None,
    )
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
    StoryLayout {
        story,
        note,
        text_box: None,
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
        no_floats: false,
        pages: None,
    };
    let (mut lps, widths, blocks) = build::build_marked(doc, &marks);
    for r in refs.iter().filter(|r| r.kind == NoteKind::Footnote) {
        let Some(i) = doc.note_index(r.kind, &r.id) else {
            continue;
        };
        let width = section_width(sections, r.section);
        let l = cached_note(cache, doc, i, width, &r.label, layout);
        let notice =
            notice_layout(doc, NoteKind::Footnote, width, cache, layout).map_or(0.0, |n| n.height);
        if let Some(lp) = lps.get_mut(r.pos.para) {
            lp.notes.push((r.pos.offset, l.height));
            lp.note_splits.push(docs_layout::NoteSplit {
                index: r.pos.offset,
                breaks: breaks(&l),
                notice,
            });
        }
    }
    (lps, widths, blocks)
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
        if doc.note_props(NoteKind::Endnote, r.section).section_end {
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
            .or_insert_with(|| story_layout(doc, i, width, ("1", "1", "1"), layout).height)
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
    let (gapped, gap_flows) = with_endnotes(
        doc,
        &refs,
        &sections,
        blocks,
        &flows,
        &mut notes_cache,
        layout,
    );
    let mut paged = PagedDocument::paginate_sections(&lps, layouts.clone(), &gapped, &gap_flows);
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
            let (gapped, gap_flows) = with_endnotes(
                doc,
                &refs,
                &sections,
                blocks,
                &flows,
                &mut notes_cache,
                layout,
            );
            paged = PagedDocument::paginate_sections(&lps, layouts, &gapped, &gap_flows);
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
    let mut in_section: HashMap<usize, usize> = HashMap::new();
    for page in &paged.pages {
        *in_section.entry(page.section).or_default() += 1;
    }
    let mut cache: HashMap<(usize, String, String, u32), Arc<StoryLayout>> = HashMap::new();
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
            let section_pages = in_section
                .get(&page.section)
                .copied()
                .unwrap_or(1)
                .to_string();
            let l = cache
                .entry((i, number.clone(), section_pages.clone(), width.to_bits()))
                .or_insert_with(|| {
                    Arc::new(story_layout(
                        doc,
                        i,
                        width,
                        (&number, &total, &section_pages),
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
    }
    let floats = place_floats(doc, &paged, &furniture, layout);
    Layout {
        paged,
        furniture,
        floats,
        strikes,
    }
}

/// A floating object's item on a page, its text box laid out.
fn float_item(
    doc: &Document,
    (story, para, index): (Option<usize>, usize, usize),
    rect: [f32; 4],
    layout: &mut LayoutFn,
) -> Option<FloatItem> {
    let owner = match story {
        Some(s) => &doc.stories().get(s)?.doc,
        None => doc,
    };
    let f = owner.float_at(para, index)?;
    let text = f.text.as_ref().map(|tb| {
        let [x, y, width, room] = floats::text_area(f, rect);
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
        let dy = match tb.v_align {
            docs_engine::float::FloatAlign::Center => ((room - sl.height) / 2.0).max(0.0),
            docs_engine::float::FloatAlign::End => (room - sl.height).max(0.0),
            _ => 0.0,
        };
        (Arc::new(sl), x, y + dy)
    });
    Some(FloatItem {
        para,
        index,
        story,
        rect,
        z: f.at.z,
        behind: f.at.behind,
        text,
    })
}

/// Every page's floating objects: the body's where the layout put them,
/// headers' and footers' from where those landed.
fn place_floats(
    doc: &Document,
    paged: &PagedDocument,
    furniture: &[Vec<Placed>],
    layout: &mut LayoutFn,
) -> Vec<Vec<FloatItem>> {
    let mut out = Vec::with_capacity(paged.pages.len());
    for (pi, page) in paged.pages.iter().enumerate() {
        let mut items = Vec::new();
        for pf in &page.floats {
            if let Some(it) = float_item(doc, (None, pf.para, pf.index), pf.rect, layout) {
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
            for (para, p) in st.doc.paragraphs().iter().enumerate() {
                for (index, o) in p.objects() {
                    let docs_engine::Object::Float(f) = o else {
                        continue;
                    };
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
                    };
                    let rect = floats::place(f, index, &page.setup, page.number, at);
                    if let Some(it) = float_item(doc, (Some(story), para, index), rect, layout) {
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
        paged: l.paged.slice(0, from, to),
        paras: l.paras.clone(),
        height: to - from,
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
) -> Vec<Vec<Placed>> {
    let mut out: Vec<Vec<Placed>> = vec![Vec::new(); paged.pages.len()];
    let find = |para: usize, index: usize| {
        refs.iter().find(|r| {
            r.kind == NoteKind::Footnote && r.pos == docs_engine::Position::new(para, index)
        })
    };
    for (pi, page) in paged.pages.iter_mut().enumerate() {
        let s = page.setup;
        if !page.note_parts.is_empty() {
            let mut y = s.height - s.margin_bottom - page.notes_height;
            let rule = if page.continued {
                // Word's continuation separator: the text column's width.
                RuleRect {
                    rect: [
                        s.margin_left,
                        y + NOTE_SEPARATOR / 2.0,
                        s.content_width(),
                        0.75,
                    ],
                    color: Rgba::BLACK,
                }
            } else {
                separator(s.margin_left, y, s.content_width())
            };
            page.rules.push(rule);
            y += NOTE_SEPARATOR;
            for part in &page.note_parts {
                let Some(r) = find(part.para, part.index) else {
                    continue;
                };
                let Some(i) = doc.note_index(r.kind, &r.id) else {
                    continue;
                };
                let width = section_width(sections, r.section);
                let l = cached_note(cache, doc, i, width, &r.label, layout);
                let l = note_part(l, part.from, part.to);
                let h = l.height;
                out[pi].push(Placed {
                    kind: HfKind::Footer,
                    layout: l,
                    x: s.margin_left,
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
                        x: s.margin_left,
                        y,
                        note: None,
                    });
                    y += h;
                }
            }
        }
        for g in &page.gaps {
            if g.id == SEPARATOR_GAP {
                page.rules
                    .push(separator(s.margin_left, g.y, s.content_width()));
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
                x: s.margin_left,
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
