//! The model → layout bridge: resolved paragraphs (indents, list markers,
//! inline objects), table blocks and the width each paragraph is broken at.
//! The Docs view and PDF export both lay out from [`build`], so the page on
//! screen and the page in the PDF are the same layout.

use docs_engine::{Align, Document, Level, Object, ResolvedLine, VMerge, font_stack};
use docs_layout::parley::LineHeight;
use docs_layout::{
    Block, InlineObject, Marker, Merge, ObjectKind, PT, PageSetup, Rgba, Rule, Span, TableBlock,
    TableCell, TableRow,
};

/// Twips to px at 96 dpi.
fn tw(t: impl Into<f64>) -> f32 {
    (t.into() / 15.0) as f32
}

/// A model colour as a layout colour.
pub fn rgba(c: docs_engine::Rgb) -> Rgba {
    let [r, g, b] = c.0;
    Rgba([r, g, b, 255])
}

/// Word-like markup colours, one per author.
const AUTHOR_COLORS: [[u8; 3]; 6] = [
    [0xC0, 0x00, 0x00],
    [0x1F, 0x5F, 0xBF],
    [0x7B, 0x2F, 0xA8],
    [0x2E, 0x7D, 0x32],
    [0xC5, 0x5A, 0x11],
    [0x00, 0x83, 0x8F],
];

/// The markup colour of a tracked change's author (stable per name).
pub fn author_color(author: &str) -> Rgba {
    let h = author
        .bytes()
        .fold(0u32, |h, b| h.wrapping_mul(31).wrapping_add(b as u32));
    let [r, g, b] = AUTHOR_COLORS[h as usize % AUTHOR_COLORS.len()];
    Rgba([r, g, b, 255])
}

/// Page size and margins from the document's (last section's) properties.
pub fn page_setup(doc: &Document) -> PageSetup {
    geometry_setup(&doc.page)
}

/// Page size and margins in px.
pub fn geometry_setup(g: &docs_engine::PageGeometry) -> PageSetup {
    PageSetup {
        width: tw(g.width),
        height: tw(g.height),
        margin_top: tw(g.margin_top),
        margin_bottom: tw(g.margin_bottom),
        margin_left: tw(g.margin_left),
        margin_right: tw(g.margin_right),
    }
}

/// The layout paragraph for a model paragraph: every run resolved, broken
/// at `width` px (the column it sits in).
pub fn layout_paragraph(
    doc: &Document,
    p: &docs_engine::Paragraph,
    marker: Option<&str>,
    width: f32,
) -> docs_layout::Paragraph {
    let rp = doc.resolve_para(p);
    let first = doc.resolve_run(p, &p.runs[0].props);
    let mut lp = docs_layout::Paragraph::new(p.text.clone());
    lp.family = font_stack(&first.font);
    lp.size_pt = first.size_pt;
    lp.line_height = Some(match rp.line {
        ResolvedLine::Multiple(m) => LineHeight::MetricsRelative(m),
        ResolvedLine::Exact(pt) => LineHeight::Absolute(pt * PT),
        // Approximation: no "at least" in parley; take the larger of the
        // minimum and a single line.
        ResolvedLine::AtLeast(pt) => LineHeight::Absolute(pt.max(first.size_pt * 1.22) * PT),
    });
    lp.default_tab = tw(doc.default_tab());
    // Stops are measured from the column edge in Word; the layout's
    // positions are too.
    lp.tabs = doc
        .tab_stops(p)
        .into_iter()
        .filter(|(pos, _)| *pos > 0)
        .map(|(pos, kind)| docs_layout::TabStop {
            pos: tw(pos as u32),
            align: match kind {
                docs_engine::TabKind::Left => docs_layout::TabAlign::Left,
                docs_engine::TabKind::Center => docs_layout::TabAlign::Center,
                docs_engine::TabKind::Right => docs_layout::TabAlign::Right,
                docs_engine::TabKind::Decimal => docs_layout::TabAlign::Decimal,
            },
        })
        .collect();
    lp.space_before_pt = rp.space_before_pt;
    lp.space_after_pt = rp.space_after_pt;
    lp.indent_left_pt = rp.indent_left_pt;
    lp.indent_right_pt = rp.indent_right_pt;
    lp.first_line_pt = rp.first_line_pt;
    lp.page_break_before = rp.page_break_before;
    lp.align = match rp.align {
        Align::Left => docs_layout::Align::Start,
        Align::Center => docs_layout::Align::Center,
        Align::Right => docs_layout::Align::End,
        Align::Justify => docs_layout::Align::Justify,
    };
    for (range, run) in p.run_ranges() {
        if range.is_empty() {
            continue;
        }
        let r = doc.resolve_run(p, &run.props);
        // Tracked changes: the author's colour, insertions underlined,
        // deletions struck through (as Word shows "All Markup").
        let (color, underline) = match &run.rev {
            Some(rev) => {
                if rev.kind == docs_engine::RevKind::Delete {
                    lp.strikes.push(range.clone());
                }
                (
                    author_color(&rev.author),
                    r.underline || rev.kind == docs_engine::RevKind::Insert,
                )
            }
            None => (rgba(r.color), r.underline),
        };
        lp.spans.push(Span {
            range,
            bold: r.bold,
            italic: r.italic,
            underline,
            size_pt: Some(r.size_pt),
            color: Some(color),
            family: Some(font_stack(&r.font)),
        });
    }
    if let Some(text) = marker.filter(|m| !m.is_empty()) {
        // The level's own font only matters for symbol bullets, which the
        // engine already mapped to Unicode: use the text's font.
        let level_font = doc.list_level(p).and_then(|l| l.font.clone()).filter(|f| {
            let f = f.to_ascii_lowercase();
            !f.contains("symbol") && !f.contains("wingdings")
        });
        lp.marker = Some(Marker {
            text: text.to_owned(),
            style: Span {
                bold: first.bold,
                italic: false,
                underline: false,
                size_pt: Some(first.size_pt),
                color: Some(rgba(first.color)),
                family: Some(font_stack(level_font.as_deref().unwrap_or(&first.font))),
                ..Span::new(0..0)
            },
        });
    }
    let text_width = (width - (rp.indent_left_pt + rp.indent_right_pt) * PT).max(16.0);
    for (index, obj) in p.objects() {
        let o = match obj {
            Object::Image(img) => {
                let (mut w, mut h) = img.size_px();
                if w > text_width && w > 0.0 {
                    h *= text_width / w;
                    w = text_width;
                }
                let drawable = doc.image_bytes(&img.rel_id).is_some();
                InlineObject {
                    index,
                    kind: if drawable {
                        ObjectKind::Image
                    } else {
                        ObjectKind::Placeholder
                    },
                    width: w.max(8.0),
                    height: h.max(8.0),
                    label: if drawable {
                        String::new()
                    } else {
                        format!("Picture ({})", img.part.rsplit('.').next().unwrap_or("?"))
                    },
                }
            }
            Object::Opaque(op) => match op.level {
                Level::Marker => InlineObject {
                    index,
                    kind: ObjectKind::Hidden,
                    width: 0.0,
                    height: 0.0,
                    label: String::new(),
                },
                Level::Block => InlineObject {
                    index,
                    kind: ObjectKind::Placeholder,
                    width: text_width,
                    height: 0.0,
                    label: op.label.clone(),
                },
                // A comment's reference mark: the comment shows in the
                // margin instead.
                Level::Run if docs_engine::model::element_name(&op.xml) == "commentReference" => {
                    InlineObject {
                        index,
                        kind: ObjectKind::Hidden,
                        width: 0.0,
                        height: 0.0,
                        label: String::new(),
                    }
                }
                Level::Run | Level::Para => InlineObject {
                    index,
                    kind: ObjectKind::Placeholder,
                    width: 0.0,
                    height: 0.0,
                    label: op.label.clone(),
                },
            },
        };
        lp.objects.push(o);
    }
    // Empty paragraphs take their height from the paragraph defaults above.
    lp.spans.retain(|s| !s.range.is_empty());
    lp
}

fn rule(b: Option<docs_engine::Border>) -> Option<Rule> {
    let b = b.filter(|b| b.on)?;
    Some(Rule {
        // Eighths of a point; at least a hairline.
        width: (b.size as f32 / 8.0 * PT).max(0.75),
        color: b.color.map(rgba).unwrap_or(Rgba::BLACK),
    })
}

/// The table block for table `id`, whose paragraphs are `range`.
fn table_block(
    doc: &Document,
    paras: &[docs_engine::Paragraph],
    id: u32,
    range: std::ops::Range<usize>,
    content_tw: u32,
) -> Option<TableBlock> {
    let t = doc.table(id)?;
    let style = doc.styles.table_style(t.style.as_deref());
    let mut tb = style.borders;
    tb.overlay(&t.borders);
    let m = t.cell_margins.or(style.cell_margins).unwrap_or_default();
    let columns: Vec<f32> = t.column_widths(content_tw).into_iter().map(tw).collect();
    let ncols = columns.len();
    let mut rows = Vec::with_capacity(t.rows.len());
    for (ri, row) in t.rows.iter().enumerate() {
        let mut cells = Vec::with_capacity(row.cells.len());
        for (ci, cell) in row.cells.iter().enumerate() {
            let want = docs_engine::CellRef {
                table: id,
                row: ri as u32,
                cell: ci as u32,
            };
            let start = range.clone().find(|&i| paras[i].cell == Some(want))?;
            let mut end = start;
            while end < range.end && paras[end].cell == Some(want) {
                end += 1;
            }
            let col = t.grid_col(ri, ci);
            let span = cell.span.max(1) as usize;
            let continues_below = |r: usize| {
                t.rows.get(r + 1).is_some_and(|next| {
                    next.cells
                        .iter()
                        .enumerate()
                        .any(|(k, c)| t.grid_col(r + 1, k) == col && c.vmerge == VMerge::Continue)
                })
            };
            let edge = |own: Option<docs_engine::Border>,
                        outer: bool,
                        o: Option<docs_engine::Border>,
                        i: Option<docs_engine::Border>| {
                own.or(if outer { o } else { i })
            };
            let mut top = edge(cell.borders.top, ri == 0, tb.top, tb.inside_h);
            let mut bottom = edge(
                cell.borders.bottom,
                ri + 1 == t.rows.len(),
                tb.bottom,
                tb.inside_h,
            );
            let left = edge(cell.borders.left, col == 0, tb.left, tb.inside_v);
            let right = edge(
                cell.borders.right,
                col + span >= ncols,
                tb.right,
                tb.inside_v,
            );
            if cell.vmerge == VMerge::Continue {
                top = None;
            }
            if cell.vmerge != VMerge::None && continues_below(ri) {
                bottom = None;
            }
            cells.push(TableCell {
                col,
                span,
                paras: start..end,
                merge: match cell.vmerge {
                    VMerge::None => Merge::None,
                    VMerge::Restart => Merge::Restart,
                    VMerge::Continue => Merge::Continue,
                },
                borders: [rule(top), rule(right), rule(bottom), rule(left)],
            });
        }
        let (min_height, exact) = row.height.map_or((0.0, false), |(h, e)| (tw(h), e));
        rows.push(TableRow {
            cells,
            min_height,
            exact,
        });
    }
    Some(TableBlock {
        x: tw(t.indent),
        columns,
        rows,
        padding: [tw(m.top), tw(m.right), tw(m.bottom), tw(m.left)],
    })
}

/// Everything layout needs: paragraphs, the width each is broken at, and
/// the block structure. Each paragraph is broken at its section's text
/// width.
pub fn build(doc: &Document) -> (Vec<docs_layout::Paragraph>, Vec<f32>, Vec<Block>) {
    let sections = doc.sections();
    let widths: Vec<u32> = sections
        .iter()
        .map(|s| {
            let g = s.props.page;
            g.width
                .saturating_sub(g.margin_left + g.margin_right)
                .max(1440)
        })
        .collect();
    let section_of = |i: usize| {
        sections
            .iter()
            .position(|s| i < s.paras.end)
            .unwrap_or(sections.len() - 1)
    };
    build_with(doc, doc.paragraphs(), &|i| widths[section_of(i)])
}

/// [`build`] over `paras` (the document's paragraphs, or a header's with
/// page numbers filled in), `content_tw(i)` giving paragraph `i`'s text
/// width in twips.
pub fn build_with(
    doc: &Document,
    paras: &[docs_engine::Paragraph],
    content_tw: &dyn Fn(usize) -> u32,
) -> (Vec<docs_layout::Paragraph>, Vec<f32>, Vec<Block>) {
    let markers = doc.list_markers();
    let mut widths: Vec<f32> = (0..paras.len()).map(|i| tw(content_tw(i))).collect();
    let mut blocks = Vec::new();
    let mut i = 0;
    while i < paras.len() {
        let table = paras[i].cell.map(|c| c.table);
        match table.map(|id| (id, table_range(paras, id))) {
            Some((id, range)) if range.start == i && !range.is_empty() => {
                match table_block(doc, paras, id, range.clone(), content_tw(i)) {
                    Some(tb) => {
                        for row in &tb.rows {
                            for cell in &row.cells {
                                let w = tb.text_width(cell);
                                for k in cell.paras.clone() {
                                    widths[k] = w;
                                }
                            }
                        }
                        blocks.push(Block::Table(tb));
                        i = range.end;
                    }
                    None => {
                        // Structure and paragraphs disagree: show the text.
                        blocks.extend(range.clone().map(Block::Para));
                        i = range.end;
                    }
                }
            }
            _ => {
                blocks.push(Block::Para(i));
                i += 1;
            }
        }
    }
    let mut out: Vec<docs_layout::Paragraph> = paras
        .iter()
        .enumerate()
        .map(|(k, p)| {
            let marker = markers.get(k).and_then(|m| m.as_deref());
            layout_paragraph(doc, p, marker, widths[k])
        })
        .collect();
    // Contextual spacing: no space between paragraphs of the same style.
    for k in 0..paras.len() {
        if !doc.resolve_para(&paras[k]).contextual {
            continue;
        }
        let same = |j: usize| {
            paras
                .get(j)
                .is_some_and(|q| q.props.style == paras[k].props.style && q.cell == paras[k].cell)
        };
        if k > 0 && same(k - 1) {
            out[k].space_before_pt = 0.0;
        }
        if same(k + 1) {
            out[k].space_after_pt = 0.0;
        }
    }
    (out, widths, blocks)
}

/// Paragraph index range of table `id` in `paras`.
fn table_range(paras: &[docs_engine::Paragraph], id: u32) -> std::ops::Range<usize> {
    let start = paras
        .iter()
        .position(|p| p.cell.is_some_and(|c| c.table == id));
    match start {
        Some(s) => {
            let len = paras[s..]
                .iter()
                .take_while(|p| p.cell.is_some_and(|c| c.table == id))
                .count();
            s..s + len
        }
        None => 0..0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use docs_engine::{ListKind, Position, Selection};

    #[std::prelude::v1::test]
    fn tables_and_lists_become_blocks_and_markers() {
        let mut doc = Document::new();
        let c = doc.insert_text(Selection::default(), "one\ntwo", None);
        doc.toggle_list(
            Selection {
                anchor: Position::new(0, 0),
                head: c.head,
            },
            ListKind::Numbered,
        );
        doc.insert_table(Selection::caret(Position::new(1, 3)), 2, 3);
        let (paras, widths, blocks) = build(&doc);
        assert_eq!(paras[0].marker.as_ref().unwrap().text, "1.");
        assert_eq!(paras[1].marker.as_ref().unwrap().text, "2.");
        assert!(paras[0].indent_left_pt > 0.0, "list indent");
        let tables: Vec<_> = blocks
            .iter()
            .filter_map(|b| match b {
                Block::Table(t) => Some(t),
                _ => None,
            })
            .collect();
        assert_eq!(tables.len(), 1);
        assert_eq!(tables[0].rows.len(), 2);
        assert_eq!(tables[0].columns.len(), 3);
        assert!(
            tables[0].rows[0].cells[0]
                .borders
                .iter()
                .all(Option::is_some)
        );
        assert!(widths[2] < widths[0] / 2.0, "cells are narrower");
        assert_eq!(
            blocks.len(),
            2 + 1 + 1,
            "two paragraphs, the table, one after"
        );
    }
}
