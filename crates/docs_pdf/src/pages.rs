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

use std::collections::HashMap;
use std::sync::Arc;

use docs_engine::{Document, HfKind, HfType, Section, SectionStart, with_page_numbers};
use docs_layout::{Block, PageSetup, PagedDocument, ParaLayout, SectionBreak, SectionFlow};

use crate::build;

/// Lays out one paragraph broken at a width (the Docs view passes its
/// cache, export the engine).
pub type LayoutFn<'a> = dyn FnMut(&docs_layout::Paragraph, f32) -> Arc<ParaLayout> + 'a;

/// A header or footer laid out for some page.
pub struct StoryLayout {
    /// Index into [`Document::stories`].
    pub story: usize,
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
}

/// A header or footer placed on a page: its layout's (0, 0) goes to
/// (`x`, `y`) in page coordinates.
#[derive(Clone)]
pub struct Placed {
    pub kind: HfKind,
    pub layout: Arc<StoryLayout>,
    pub x: f32,
    pub y: f32,
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
    let width_tw = (width * 15.0).round() as u32;
    let (lps, widths, blocks) = build::build_with(sd, &paras, &|_| width_tw);
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
        story: index,
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

/// Lay a document out on pages: sections, headers and footers.
pub fn lay_out_with(doc: &Document, layout: &mut LayoutFn) -> Layout {
    let sections = doc.sections();
    let (lps, widths, blocks) = build::build(doc);
    let layouts: Vec<Arc<ParaLayout>> = lps
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
    let paged = PagedDocument::paginate_sections(&lps, layouts, &blocks, &flows);
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
            });
        }
        furniture.push(placed);
    }
    Layout {
        paged,
        furniture,
        strikes,
    }
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
