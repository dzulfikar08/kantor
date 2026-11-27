//! Markdown → the live-preview line model.
//!
//! [`parse`] runs pulldown-cmark (CommonMark + GFM tables, task lists,
//! strikethrough, footnotes, YAML front matter, `[[wiki links]]`) with
//! source offsets and turns its events into one [`LineInfo`] per buffer
//! line. Nothing is ever re-serialised: the model only says, for each
//! byte range of the source, how to draw it.
//!
//! The rule that hides the syntax is simple: in a recognised block, the
//! bytes covered by *content* events (text, inline code without its
//! backticks, HTML, code-block text) are visible and every other byte is
//! markup (`**`, `## `, `- `, `> `, `](url)`, table pipes, fences). A line
//! the editor *reveals* (the caret is on it) shows all its bytes, markup
//! muted. A line in no recognised block (a link reference definition)
//! always shows as typed.

use std::ops::Range;

use pulldown_cmark::{CodeBlockKind, Event, LinkType, Options, Parser, Tag, TagEnd};

use crate::buffer::Buffer;

/// What a line is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LineKind {
    /// Empty, or only markup (a `>` alone in a quote).
    Blank,
    /// In no block the parser recognised: shown as typed.
    Raw,
    Paragraph,
    /// An ATX heading, or a setext heading's text line (level 1–6).
    Heading(u8),
    /// The `===` / `---` line under a setext heading.
    SetextUnderline(u8),
    /// A fenced code block's opening (`open`) or closing fence.
    CodeFence {
        block: usize,
        open: bool,
    },
    /// A line of a code block.
    Code {
        block: usize,
    },
    /// A table row (`row` 0 is the header).
    TableRow {
        table: usize,
        row: usize,
    },
    /// The `|---|:-:|` line under a table's header.
    TableDelimiter {
        table: usize,
    },
    /// `---`, `***`, `___`.
    Rule,
    /// An HTML block.
    Html,
    /// YAML front matter between `---` lines (`delimiter`: one of those).
    FrontMatter {
        delimiter: bool,
    },
}

/// What a list item's first line draws before its text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ListMarker {
    None,
    Bullet,
    /// `1.` / `3)`: the number as written (source range).
    Ordered(Range<usize>),
    /// `- [ ]` / `- [x]`: the `[ ]` range.
    Task {
        checked: bool,
        at: Range<usize>,
    },
}

/// Inline styles (spans can nest and overlap a line's edges).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpanKind {
    Strong,
    Emphasis,
    Strike,
    Code,
    /// Index into [`Parsed::links`].
    Link(usize),
    /// Index into [`Parsed::images`].
    Image(usize),
    Html,
    Footnote,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Span {
    pub range: Range<usize>,
    pub kind: SpanKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LinkKind {
    /// `[text](url)`, references, `<url>` autolinks and e-mail links.
    Url,
    /// `[[Note]]` / `[[Note|label]]` (resolved against the note's folder in
    /// a later phase).
    Wiki,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Link {
    pub range: Range<usize>,
    pub dest: String,
    pub kind: LinkKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Image {
    pub range: Range<usize>,
    pub dest: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    None,
    Left,
    Center,
    Right,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TableRow {
    pub line: usize,
    /// Each cell's source range (its text, trimmed by the parser).
    pub cells: Vec<Range<usize>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Table {
    /// Buffer lines of the table, delimiter row included.
    pub lines: Range<usize>,
    pub aligns: Vec<Align>,
    /// Header first.
    pub rows: Vec<TableRow>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodeBlock {
    pub lines: Range<usize>,
    /// The info string's first word (`rust` in ```` ```rust ````).
    pub lang: String,
    pub fenced: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Heading {
    pub level: u8,
    pub line: usize,
    /// The heading's text without its markup.
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LineInfo {
    pub kind: LineKind,
    /// Where the line's content starts: before it is markup only (`## `,
    /// `> - [ ] `, indentation), drawn as decorations when hidden.
    pub content_start: usize,
    /// Block-quote depth.
    pub quote: u8,
    /// How many lists hold the line.
    pub list_depth: u8,
    pub marker: ListMarker,
    /// The lines revealed together with this one (a table, a code block,
    /// a setext heading, the front matter).
    pub group: Range<usize>,
    /// Visible (content) byte ranges in the line, in order.
    pub visible: Vec<Range<usize>>,
    /// Inline styles over the line, clipped to it.
    pub spans: Vec<Span>,
}

#[derive(Clone, Debug, Default)]
pub struct Parsed {
    pub lines: Vec<LineInfo>,
    pub links: Vec<Link>,
    pub images: Vec<Image>,
    pub tables: Vec<Table>,
    pub code_blocks: Vec<CodeBlock>,
    pub headings: Vec<Heading>,
}

pub fn options() -> Options {
    Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_YAML_STYLE_METADATA_BLOCKS
        | Options::ENABLE_WIKILINKS
}

/// Lines `r` touches (`r.end` exclusive; an empty range touches its line).
fn lines_of(buf: &Buffer, r: &Range<usize>) -> Range<usize> {
    let first = buf.line_of(r.start);
    let last = buf.line_of(r.end.max(r.start + 1).saturating_sub(1).max(r.start));
    first..last + 1
}

fn level(l: pulldown_cmark::HeadingLevel) -> u8 {
    l as u8
}

struct Frame {
    range: Range<usize>,
    kind: SpanKind,
}

pub fn parse(buf: &Buffer) -> Parsed {
    let text = buf.text();
    let n = buf.line_count();
    let mut kinds: Vec<Option<LineKind>> = vec![None; n];
    let mut quote = vec![0u8; n];
    let mut list_depth = vec![0u8; n];
    let mut markers = vec![ListMarker::None; n];
    let mut groups: Vec<Range<usize>> = (0..n).map(|i| i..i + 1).collect();
    let mut covered: Vec<Range<usize>> = Vec::new();
    let mut spans: Vec<Span> = Vec::new();
    let mut out = Parsed::default();

    let mut frames: Vec<Frame> = Vec::new();
    let mut lists: Vec<Option<u64>> = Vec::new();
    let mut heading: Option<(u8, Range<usize>)> = None;
    let mut heading_text = String::new();
    let mut table: Option<Table> = None;
    let mut row: Option<TableRow> = None;

    let set = |kinds: &mut Vec<Option<LineKind>>, lines: Range<usize>, k: LineKind| {
        for l in lines {
            if l < kinds.len() && kinds[l].is_none() {
                kinds[l] = Some(k.clone());
            }
        }
    };

    for (event, r) in Parser::new_ext(text, options()).into_offset_iter() {
        match event {
            Event::Start(tag) => match tag {
                Tag::Paragraph => set(&mut kinds, lines_of(buf, &r), LineKind::Paragraph),
                Tag::Heading { level: lv, .. } => {
                    let lv = level(lv);
                    let lines = lines_of(buf, &r);
                    let last = lines.end - 1;
                    let under = buf.line(last).trim();
                    let setext = lines.len() > 1
                        && !under.is_empty()
                        && (under.chars().all(|c| c == '=') || under.chars().all(|c| c == '-'));
                    if setext {
                        set(&mut kinds, lines.start..last, LineKind::Heading(lv));
                        set(&mut kinds, last..last + 1, LineKind::SetextUnderline(lv));
                        for l in lines.clone() {
                            groups[l] = lines.clone();
                        }
                    } else {
                        set(&mut kinds, lines.clone(), LineKind::Heading(lv));
                    }
                    heading = Some((lv, r.clone()));
                    heading_text.clear();
                }
                Tag::BlockQuote(_) => {
                    for l in lines_of(buf, &r) {
                        quote[l] = quote[l].saturating_add(1);
                    }
                }
                Tag::CodeBlock(kind) => {
                    let lines = lines_of(buf, &r);
                    let block = out.code_blocks.len();
                    let (fenced, lang) = match &kind {
                        CodeBlockKind::Fenced(info) => (
                            true,
                            info.split_whitespace().next().unwrap_or("").to_owned(),
                        ),
                        CodeBlockKind::Indented => (false, String::new()),
                    };
                    let mut body = lines.clone();
                    if fenced {
                        set(
                            &mut kinds,
                            lines.start..lines.start + 1,
                            LineKind::CodeFence { block, open: true },
                        );
                        body.start += 1;
                        let last = lines.end - 1;
                        let t = buf.line(last).trim_start_matches([' ', '>', '\t']);
                        if last > lines.start && (t.starts_with("```") || t.starts_with("~~~")) {
                            set(
                                &mut kinds,
                                last..last + 1,
                                LineKind::CodeFence { block, open: false },
                            );
                            body.end = last;
                        }
                    }
                    set(&mut kinds, body, LineKind::Code { block });
                    for l in lines.clone() {
                        groups[l] = lines.clone();
                    }
                    out.code_blocks.push(CodeBlock {
                        lines,
                        lang,
                        fenced,
                    });
                }
                Tag::HtmlBlock => set(&mut kinds, lines_of(buf, &r), LineKind::Html),
                Tag::MetadataBlock(_) => {
                    let lines = lines_of(buf, &r);
                    for l in lines.clone() {
                        let delimiter = l == lines.start || l == lines.end - 1;
                        set(&mut kinds, l..l + 1, LineKind::FrontMatter { delimiter });
                        groups[l] = lines.clone();
                    }
                }
                Tag::List(start) => {
                    for l in lines_of(buf, &r) {
                        list_depth[l] = list_depth[l].saturating_add(1);
                    }
                    lists.push(start);
                }
                Tag::Item => {
                    let l = buf.line_of(r.start);
                    let ordered = lists.last().copied().flatten().is_some();
                    markers[l] = if ordered {
                        let s = &text[r.start..buf.line_range(l).end];
                        let len = s.bytes().take_while(u8::is_ascii_digit).count();
                        let len =
                            len + usize::from(matches!(s.as_bytes().get(len), Some(b'.' | b')')));
                        ListMarker::Ordered(r.start..r.start + len)
                    } else {
                        ListMarker::Bullet
                    };
                }
                Tag::Table(aligns) => {
                    let lines = lines_of(buf, &r);
                    for l in lines.clone() {
                        groups[l] = lines.clone();
                    }
                    table = Some(Table {
                        lines,
                        aligns: aligns
                            .iter()
                            .map(|a| match a {
                                pulldown_cmark::Alignment::None => Align::None,
                                pulldown_cmark::Alignment::Left => Align::Left,
                                pulldown_cmark::Alignment::Center => Align::Center,
                                pulldown_cmark::Alignment::Right => Align::Right,
                            })
                            .collect(),
                        rows: Vec::new(),
                    });
                }
                Tag::TableHead | Tag::TableRow => {
                    row = Some(TableRow {
                        line: buf.line_of(r.start),
                        cells: Vec::new(),
                    });
                }
                Tag::TableCell => {
                    if let Some(row) = &mut row {
                        row.cells.push(r.clone());
                    }
                }
                Tag::Emphasis => frames.push(Frame {
                    range: r,
                    kind: SpanKind::Emphasis,
                }),
                Tag::Strong => frames.push(Frame {
                    range: r,
                    kind: SpanKind::Strong,
                }),
                Tag::Strikethrough => frames.push(Frame {
                    range: r,
                    kind: SpanKind::Strike,
                }),
                Tag::Link {
                    link_type,
                    dest_url,
                    ..
                } => {
                    let i = out.links.len();
                    out.links.push(Link {
                        range: r.clone(),
                        dest: dest_url.to_string(),
                        kind: match link_type {
                            LinkType::WikiLink { .. } => LinkKind::Wiki,
                            _ => LinkKind::Url,
                        },
                    });
                    frames.push(Frame {
                        range: r,
                        kind: SpanKind::Link(i),
                    });
                }
                Tag::Image { dest_url, .. } => {
                    let i = out.images.len();
                    out.images.push(Image {
                        range: r.clone(),
                        dest: dest_url.to_string(),
                    });
                    frames.push(Frame {
                        range: r,
                        kind: SpanKind::Image(i),
                    });
                }
                _ => {}
            },
            Event::End(end) => match end {
                TagEnd::Emphasis
                | TagEnd::Strong
                | TagEnd::Strikethrough
                | TagEnd::Link
                | TagEnd::Image => {
                    if let Some(f) = frames.pop() {
                        spans.push(Span {
                            range: f.range,
                            kind: f.kind,
                        });
                    }
                }
                TagEnd::Heading(_) => {
                    if let Some((lv, r)) = heading.take() {
                        out.headings.push(Heading {
                            level: lv,
                            line: buf.line_of(r.start),
                            text: std::mem::take(&mut heading_text),
                        });
                    }
                }
                TagEnd::List(_) => {
                    lists.pop();
                }
                TagEnd::TableHead | TagEnd::TableRow => {
                    if let (Some(t), Some(row)) = (&mut table, row.take()) {
                        t.rows.push(row);
                    }
                }
                TagEnd::Table => {
                    if let Some(t) = table.take() {
                        let ti = out.tables.len();
                        for (ri, row) in t.rows.iter().enumerate() {
                            kinds[row.line] = Some(LineKind::TableRow { table: ti, row: ri });
                        }
                        if let Some(head) = t.rows.first()
                            && head.line + 1 < t.lines.end
                        {
                            kinds[head.line + 1] = Some(LineKind::TableDelimiter { table: ti });
                        }
                        out.tables.push(t);
                    }
                }
                _ => {}
            },
            Event::Text(s) => {
                if heading.is_some() {
                    heading_text.push_str(&s);
                }
                covered.push(r);
            }
            Event::Code(s) => {
                if heading.is_some() {
                    heading_text.push_str(&s);
                }
                let src = &text[r.clone()];
                let open = src.bytes().take_while(|&b| b == b'`').count();
                let close = src.bytes().rev().take_while(|&b| b == b'`').count();
                if open + close <= src.len() {
                    covered.push(r.start + open..r.end - close);
                }
                spans.push(Span {
                    range: r,
                    kind: SpanKind::Code,
                });
            }
            Event::Html(_) | Event::InlineHtml(_) => {
                covered.push(r.clone());
                spans.push(Span {
                    range: r,
                    kind: SpanKind::Html,
                });
            }
            Event::FootnoteReference(_) => {
                covered.push(r.clone());
                spans.push(Span {
                    range: r,
                    kind: SpanKind::Footnote,
                });
            }
            Event::InlineMath(_) | Event::DisplayMath(_) => covered.push(r),
            Event::Rule => set(&mut kinds, lines_of(buf, &r), LineKind::Rule),
            Event::TaskListMarker(checked) => {
                let l = buf.line_of(r.start);
                markers[l] = ListMarker::Task {
                    checked,
                    at: r.start..r.start + 3,
                };
            }
            Event::SoftBreak | Event::HardBreak => {}
        }
    }

    covered.sort_by_key(|r| (r.start, r.end));
    spans.sort_by_key(|s| (s.range.start, s.range.end));

    // Per line: visible ranges and spans, clipped.
    let mut ci = 0;
    let mut lines = Vec::with_capacity(n);
    for l in 0..n {
        let lr = buf.line_range(l);
        while ci < covered.len() && covered[ci].end <= lr.start && covered[ci].start < lr.start {
            ci += 1;
        }
        let mut visible = Vec::new();
        let mut j = ci;
        while j < covered.len() && covered[j].start <= lr.end {
            let c = &covered[j];
            let (s, e) = (c.start.max(lr.start), c.end.min(lr.end));
            if s < e {
                visible.push(s..e);
            }
            j += 1;
        }
        let quote_l = quote[l];
        let kind = match kinds[l].take() {
            Some(k) => k,
            // Text of a tight list item (no paragraph events).
            None if !visible.is_empty() => LineKind::Paragraph,
            None if quote_l > 0 || list_depth[l] > 0 || text[lr.clone()].trim().is_empty() => {
                LineKind::Blank
            }
            None => LineKind::Raw,
        };
        let content_start = match kind {
            LineKind::Raw => lr.start,
            LineKind::Blank | LineKind::Rule | LineKind::SetextUnderline(_) => lr.end,
            LineKind::CodeFence { .. } | LineKind::FrontMatter { delimiter: true } => {
                // The fence itself is the content (`lang` shows on it).
                quote_prefix_end(&text[lr.clone()], quote_l) + lr.start
            }
            _ => visible.first().map_or(lr.end, |v| v.start),
        };
        lines.push(LineInfo {
            kind,
            content_start,
            quote: quote_l,
            list_depth: list_depth[l],
            marker: std::mem::replace(&mut markers[l], ListMarker::None),
            group: groups[l].clone(),
            visible,
            spans: Vec::new(),
        });
    }
    for s in &spans {
        for l in lines_of(buf, &s.range) {
            let lr = buf.line_range(l);
            let (a, b) = (s.range.start.max(lr.start), s.range.end.min(lr.end));
            if a < b || (a == b && s.range.is_empty()) {
                lines[l].spans.push(Span {
                    range: a..b,
                    kind: s.kind,
                });
            }
        }
    }
    out.lines = lines;
    out
}

/// Bytes of `line` taken by `depth` block-quote markers (`> > `).
pub fn quote_prefix_end(line: &str, depth: u8) -> usize {
    let b = line.as_bytes();
    let mut i = 0;
    for _ in 0..depth {
        let mut j = i;
        while j < b.len() && j - i < 3 && b[j] == b' ' {
            j += 1;
        }
        if b.get(j) != Some(&b'>') {
            break;
        }
        j += 1;
        if b.get(j) == Some(&b' ') {
            j += 1;
        }
        i = j;
    }
    i
}

impl Parsed {
    /// The link whose source range holds `offset`.
    pub fn link_at(&self, offset: usize) -> Option<&Link> {
        self.links
            .iter()
            .find(|l| l.range.start <= offset && offset < l.range.end)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> (Buffer, Parsed) {
        let b = Buffer::new(s);
        let parsed = parse(&b);
        (b, parsed)
    }

    /// The line as drawn when not revealed.
    fn shown(b: &Buffer, parsed: &Parsed, line: usize) -> String {
        parsed.lines[line]
            .visible
            .iter()
            .map(|r| b.slice(r.clone()))
            .collect()
    }

    #[test]
    fn inline_markup_is_hidden() {
        let (b, x) = p("Some **bold**, *it*, `code` and ~~gone~~.");
        assert_eq!(shown(&b, &x, 0), "Some bold, it, code and gone.");
        let kinds: Vec<_> = x.lines[0].spans.iter().map(|s| s.kind).collect();
        assert_eq!(
            kinds,
            [
                SpanKind::Strong,
                SpanKind::Emphasis,
                SpanKind::Code,
                SpanKind::Strike
            ]
        );
    }

    #[test]
    fn headings_and_setext() {
        let (b, x) = p("## Title ##\n\nSub\n===\n");
        assert_eq!(x.lines[0].kind, LineKind::Heading(2));
        assert_eq!(shown(&b, &x, 0), "Title");
        assert_eq!(x.lines[0].content_start, 3);
        assert_eq!(x.lines[2].kind, LineKind::Heading(1));
        assert_eq!(x.lines[3].kind, LineKind::SetextUnderline(1));
        assert_eq!(x.lines[3].group, 2..4);
        assert_eq!(x.headings[0].text, "Title");
        assert_eq!(x.headings[1].line, 2);
    }

    #[test]
    fn lists_and_tasks() {
        let (b, x) = p("- one\n  - two\n1. three\n- [x] done\n- [ ] todo\n");
        assert_eq!(x.lines[0].marker, ListMarker::Bullet);
        assert_eq!(x.lines[0].list_depth, 1);
        assert_eq!(x.lines[1].list_depth, 2);
        assert_eq!(shown(&b, &x, 1), "two");
        assert!(matches!(&x.lines[2].marker, ListMarker::Ordered(r) if b.slice(r.clone()) == "1."));
        match &x.lines[3].marker {
            ListMarker::Task { checked, at } => {
                assert!(checked);
                assert_eq!(b.slice(at.clone()), "[x]");
            }
            m => panic!("{m:?}"),
        }
        assert_eq!(shown(&b, &x, 4), "todo");
    }

    #[test]
    fn quotes_code_and_rules() {
        let (b, x) = p("> quoted\n> more\n\n```rust\nlet x = 1;\n```\n\n---\n");
        assert_eq!(x.lines[0].quote, 1);
        assert_eq!(shown(&b, &x, 1), "more");
        assert_eq!(
            x.lines[3].kind,
            LineKind::CodeFence {
                block: 0,
                open: true
            }
        );
        assert_eq!(x.lines[4].kind, LineKind::Code { block: 0 });
        assert_eq!(shown(&b, &x, 4), "let x = 1;");
        assert_eq!(
            x.lines[5].kind,
            LineKind::CodeFence {
                block: 0,
                open: false
            }
        );
        assert_eq!(x.code_blocks[0].lang, "rust");
        assert_eq!(x.lines[4].group, 3..6);
        assert_eq!(x.lines[7].kind, LineKind::Rule);
    }

    #[test]
    fn tables() {
        let (b, x) = p("| a | b |\n|---|--:|\n| 1 | **2** |\n");
        let t = &x.tables[0];
        assert_eq!(t.lines, 0..3);
        assert_eq!(t.aligns, [Align::None, Align::Right]);
        assert_eq!(t.rows.len(), 2);
        assert_eq!(x.lines[1].kind, LineKind::TableDelimiter { table: 0 });
        assert_eq!(x.lines[2].kind, LineKind::TableRow { table: 0, row: 1 });
        assert_eq!(b.slice(t.rows[1].cells[0].clone()).trim(), "1");
    }

    #[test]
    fn links_images_and_wiki_links() {
        let (b, x) =
            p("See [site](https://a.b) and [[Other Note|other]].\n\n![cat](img/cat.png)\n");
        assert_eq!(shown(&b, &x, 0), "See site and other.");
        assert_eq!(x.links[0].dest, "https://a.b");
        assert_eq!(x.links[1].kind, LinkKind::Wiki);
        assert_eq!(x.links[1].dest, "Other Note");
        assert_eq!(x.images[0].dest, "img/cat.png");
        assert_eq!(shown(&b, &x, 2), "cat");
    }

    #[test]
    fn reference_definitions_and_front_matter_stay_visible() {
        let (b, x) = p("---\ntitle: x\n---\n\n[a]: https://x.y\n");
        assert_eq!(x.lines[0].kind, LineKind::FrontMatter { delimiter: true });
        assert_eq!(x.lines[1].kind, LineKind::FrontMatter { delimiter: false });
        assert_eq!(x.lines[4].kind, LineKind::Raw);
        assert_eq!(x.lines[4].content_start, b.line_start(4));
    }

    #[test]
    fn crlf_lines() {
        let (b, x) = p("# A\r\n\r\n**b**\r\n");
        assert_eq!(x.lines[0].kind, LineKind::Heading(1));
        assert_eq!(shown(&b, &x, 2), "b");
        assert_eq!(x.lines.len(), b.line_count());
    }
}
