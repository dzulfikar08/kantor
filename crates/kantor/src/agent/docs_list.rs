//! `kantor docs list`: a document's outline as JSON (sections, headings,
//! styles, tables, pictures, fields, comments, changes, notes, pages), and
//! its paragraphs (`--text`). Paragraph ids are paragraph indices from 0,
//! counting table cells' paragraphs in order (the MCP docs tools' indices).

use std::collections::BTreeMap;

use docs_engine::object::Object;
use docs_engine::{Document, FloatKind, Paragraph, Position};
use serde_json::{Value, json};

/// Laid-out pages: the page (from 0) each paragraph starts on.
pub struct Pages {
    pub count: usize,
    pub of_para: Vec<Option<usize>>,
    pub laid: docs_pdf::Layout,
}

impl Pages {
    pub fn of(doc: &Document) -> Self {
        let mut engine = docs_layout::Engine::new();
        let laid = docs_pdf::lay_out(doc, &mut engine);
        let of_para = (0..doc.len())
            .map(|i| laid.paged.placed_line(i, 0).map(|(p, _)| p))
            .collect();
        Self {
            count: laid.paged.pages.len(),
            of_para,
            laid,
        }
    }

    /// The page number (from 1) paragraph `i` starts on.
    pub fn page(&self, i: usize) -> Option<usize> {
        self.of_para.get(i).copied().flatten().map(|p| p + 1)
    }
}

/// The text of paragraph `i` as shown (objects as `[picture]`, `[chart]`,
/// `[object]`; markers and field marks left out).
pub fn shown(doc: &Document, i: usize) -> String {
    let p = doc.paragraph(i);
    let mut out = String::with_capacity(p.text.len());
    for (offset, c) in p.text.char_indices() {
        if c != docs_engine::OBJECT {
            out.push(if c == docs_engine::LINE_BREAK {
                '\n'
            } else {
                c
            });
            continue;
        }
        match p.object_at(offset) {
            Some(o) if o.is_zero_width() => {}
            Some(Object::Image(_)) => out.push_str("[picture]"),
            Some(Object::Float(f)) if matches!(f.kind, FloatKind::Chart { .. }) => {
                out.push_str("[chart]")
            }
            Some(Object::Float(f)) if matches!(f.kind, FloatKind::Picture { .. }) => {
                out.push_str("[picture]")
            }
            Some(Object::Opaque(o)) if !o.label.is_empty() => {
                out.push('[');
                out.push_str(&o.label);
                out.push(']');
            }
            _ => out.push_str("[object]"),
        }
    }
    out
}

/// The style's display name of a paragraph.
pub fn style_name(doc: &Document, p: &Paragraph) -> String {
    p.props
        .style
        .as_deref()
        .map_or_else(|| "Normal".to_owned(), |s| doc.styles.display_name(s))
}

/// A paragraph's heading level (1–9): heading styles by name, or an
/// outline level from its style chain or itself.
pub fn heading_level(doc: &Document, p: &Paragraph) -> Option<u8> {
    let outline = |extras: &docs_engine::model::Extras| -> Option<u8> {
        extras.iter().find_map(|x| {
            (docs_engine::model::element_name(x) == "outlineLvl")
                .then(|| docs_engine::xml::parse(x)?.num::<u8>("val"))
                .flatten()
        })
    };
    if let Some(l) = outline(&p.props.extras) {
        return (l < 9).then_some(l + 1);
    }
    let mut id = p.props.style.clone();
    for _ in 0..12 {
        let s = doc.styles.get(id.as_deref()?)?;
        let name = s.name.to_ascii_lowercase();
        if let Some(n) = name
            .strip_prefix("heading ")
            .and_then(|n| n.trim().parse::<u8>().ok())
        {
            return (1..=9).contains(&n).then_some(n);
        }
        if let Some(l) = outline(&s.para.extras) {
            return (l < 9).then_some(l + 1);
        }
        id = s.based_on.clone();
    }
    None
}

/// Whether run `r` has character formatting of its own (beyond a
/// character style and language marks).
pub fn direct(r: &docs_engine::Run) -> bool {
    let p = &r.props;
    !p.fonts.is_empty()
        || p.size.is_some()
        || p.bold.is_some()
        || p.italic.is_some()
        || p.underline.is_some()
        || p.color.is_some()
        || p.extras.iter().any(|x| {
            !matches!(
                docs_engine::model::element_name(x),
                "lang"
                    | "noProof"
                    | "rtl"
                    | "cs"
                    | "rStyle"
                    | "webHidden"
                    | "szCs"
                    | "bCs"
                    | "iCs"
                    | "rPrChange"
            )
        })
}

fn cm(tw: u32) -> f64 {
    (tw as f64 / 1440.0 * 2.54 * 100.0).round() / 100.0
}

fn kind_name(k: docs_engine::RevKind) -> &'static str {
    match k {
        docs_engine::RevKind::Insert => "insert",
        docs_engine::RevKind::Delete => "delete",
        docs_engine::RevKind::Format => "format",
    }
}

/// The document's outline.
pub fn outline(doc: &Document) -> Value {
    let pages = Pages::of(doc);
    let markers = doc.list_markers();
    let n = doc.len();

    // Sections.
    let sections: Vec<Value> = doc
        .sections()
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let g = &s.props.page;
            json!({
                "section": i + 1,
                "paragraphs": [s.paras.start, s.paras.end.saturating_sub(1)],
                "page": pages.page(s.paras.start),
                "size_cm": [cm(g.width), cm(g.height)],
                "orientation": if g.width > g.height { "landscape" } else { "portrait" },
                "margins_cm": {"top": cm(g.margin_top), "bottom": cm(g.margin_bottom), "left": cm(g.margin_left), "right": cm(g.margin_right)},
                "columns": s.props.columns.count(),
                "different_first_page": s.props.title_page,
                "page_numbers_start": s.props.page_start,
            })
        })
        .collect();

    // Headings as a tree.
    let mut flat: Vec<(u8, Value)> = Vec::new();
    for i in 0..n {
        let p = doc.paragraph(i);
        if p.cell.is_some() {
            continue;
        }
        if let Some(level) = heading_level(doc, p) {
            let mut v = json!({"id": i, "level": level, "text": shown(doc, i).trim(), "style": style_name(doc, p), "page": pages.page(i)});
            if let Some(m) = markers.get(i).cloned().flatten() {
                v["number"] = json!(m.trim().trim_end_matches(['.', ')']));
            }
            flat.push((level, v));
        }
    }
    fn tree(items: &[(u8, Value)]) -> Vec<Value> {
        let mut out = Vec::new();
        let mut i = 0;
        while i < items.len() {
            let (lvl, mut v) = items[i].clone();
            let mut j = i + 1;
            while j < items.len() && items[j].0 > lvl {
                j += 1;
            }
            let children = tree(&items[i + 1..j]);
            if !children.is_empty() {
                v["children"] = json!(children);
            }
            out.push(v);
            i = j;
        }
        out
    }

    // Styles and direct formatting.
    let mut styles: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    let mut direct_paras: Vec<usize> = Vec::new();
    let mut direct_runs = 0;
    for i in 0..n {
        let p = doc.paragraph(i);
        let e = styles.entry(style_name(doc, p)).or_default();
        e.0 += 1;
        let d = p
            .runs
            .iter()
            .filter(|r| r.object.is_none() && r.len > 0 && direct(r))
            .count();
        if d > 0 {
            e.1 += 1;
            direct_runs += d;
            direct_paras.push(i);
        }
    }
    let styles: Vec<Value> = styles
        .into_iter()
        .map(
            |(k, (count, d))| json!({"style": k, "paragraphs": count, "with_direct_formatting": d}),
        )
        .collect();

    // Tables, pictures, charts.
    let mut tables = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut pictures = Vec::new();
    let mut charts = Vec::new();
    for i in 0..n {
        let p = doc.paragraph(i);
        if let Some(c) = p.cell
            && seen.insert(c.table)
            && let Some(t) = doc.table(c.table)
        {
            let range = doc.table_range(c.table);
            let caption = [i.checked_sub(1), Some(range.end)]
                .into_iter()
                .flatten()
                .filter(|&k| k < n)
                .find(|&k| style_name(doc, doc.paragraph(k)).eq_ignore_ascii_case("caption"))
                .map(|k| shown(doc, k));
            tables.push(json!({
                "id": i,
                "paragraphs": [range.start, range.end.saturating_sub(1)],
                "rows": t.rows.len(),
                "columns": t.grid.len(),
                "header_rows": t.rows.iter().take_while(|r| r.header).count(),
                "caption": caption,
                "page": pages.page(i),
            }));
        }
        for (_, o) in p.objects() {
            match o {
                Object::Image(img) => pictures.push(json!({
                    "id": i, "alt": img.descr, "inline": true,
                    "size_cm": [((img.cx as f64) / 360_000.0 * 100.0).round() / 100.0, ((img.cy as f64) / 360_000.0 * 100.0).round() / 100.0],
                    "page": pages.page(i),
                })),
                Object::Float(f) => match &f.kind {
                    FloatKind::Chart { .. } => charts.push(json!({"id": i, "alt": f.descr, "inline": f.inline, "page": pages.page(i)})),
                    FloatKind::Picture { .. } => pictures.push(json!({"id": i, "alt": f.descr, "inline": false, "page": pages.page(i)})),
                    _ => {}
                },
                _ => {}
            }
        }
    }

    // Fields.
    let mut field_counts: BTreeMap<String, usize> = BTreeMap::new();
    let mut fields = Vec::new();
    for f in doc.fields() {
        let k = f.kind();
        *field_counts.entry(k.clone()).or_default() += 1;
        if matches!(k.as_str(), "TOC" | "REF" | "PAGEREF" | "SEQ" | "NOTEREF") && fields.len() < 300
        {
            let (a, b) = f.result();
            let result: String = doc
                .text_in(a, b)
                .replace('\u{FFFC}', "")
                .chars()
                .take(120)
                .collect();
            fields.push(
                json!({"type": k, "id": f.begin.para, "code": f.instr.trim(), "result": result}),
            );
        }
    }

    // Comments, changes, notes.
    let anchors = doc.comment_anchors();
    let comments: Vec<Value> = doc
        .comments()
        .iter()
        .map(|c| {
            let at = anchors.iter().find(|(id, _)| *id == c.id).map(|(_, a)| a.start.para);
            json!({"id": c.id, "author": c.author, "text": c.text, "paragraph": at, "resolved": c.done, "reply_to": c.parent})
        })
        .collect();
    let changes: Vec<Value> = doc
        .changes()
        .iter()
        .take(500)
        .map(|c| {
            json!({"kind": kind_name(c.kind), "author": c.author, "date": c.date, "id": c.start.para,
                   "text": c.text.chars().take(200).collect::<String>()})
        })
        .collect();
    let notes: Vec<Value> = doc
        .note_refs()
        .iter()
        .map(|r| {
            let text = doc
                .notes()
                .iter()
                .find(|n| n.id == r.id && n.kind == r.kind)
                .map(|n| {
                    (0..n.doc.len())
                        .map(|k| shown(&n.doc, k))
                        .collect::<Vec<_>>()
                        .join("\n")
                })
                .unwrap_or_default();
            json!({
                "kind": match r.kind { docs_engine::NoteKind::Footnote => "footnote", docs_engine::NoteKind::Endnote => "endnote" },
                "number": r.label, "id": r.pos.para, "text": text.trim(),
            })
        })
        .collect();

    json!({
        "paragraphs": n,
        "pages": pages.count,
        "words": doc.word_count(),
        "sections": sections,
        "headings": tree(&flat),
        "styles": styles,
        "direct_formatting": {"runs": direct_runs, "paragraphs": direct_paras.iter().take(50).collect::<Vec<_>>()},
        "tables": tables,
        "pictures": pictures,
        "charts": charts,
        "field_counts": field_counts,
        "fields": fields,
        "comments": comments,
        "tracked_changes": changes,
        "notes": notes,
        "track_changes_on": doc.track_changes(),
    })
}

/// Paragraphs `start..start + limit`: id, style, text, list number, table
/// cell, page.
pub fn text(doc: &Document, start: usize, limit: usize) -> Value {
    let pages = Pages::of(doc);
    let markers = doc.list_markers();
    let end = start.saturating_add(limit.clamp(1, 5000)).min(doc.len());
    let paras: Vec<Value> = (start.min(end)..end)
        .map(|i| {
            let p = doc.paragraph(i);
            let mut v = json!({"id": i, "style": style_name(doc, p), "text": shown(doc, i), "page": pages.page(i)});
            if let Some(m) = markers.get(i).cloned().flatten() {
                v["number"] = json!(m.trim());
            }
            if let Some(l) = heading_level(doc, p) {
                v["heading"] = json!(l);
            }
            if let Some(c) = p.cell {
                v["table"] = json!({"row": c.row, "cell": c.cell, "first": doc.table_range(c.table).start});
            }
            v
        })
        .collect();
    let mut out = json!({"total": doc.len(), "pages": pages.count, "paragraphs": paras});
    if end < doc.len() {
        out["next_start"] = json!(end);
    }
    out
}

/// Where paragraph `i` (a heading) ends its section: the next heading of
/// its level or higher, or the end.
pub fn section_end(doc: &Document, i: usize) -> usize {
    let level = heading_level(doc, doc.paragraph(i)).unwrap_or(1);
    (i + 1..doc.len())
        .find(|&k| {
            let p = doc.paragraph(k);
            p.cell.is_none() && heading_level(doc, p).is_some_and(|l| l <= level)
        })
        .unwrap_or(doc.len())
}

/// The heading `query` names: its text (any case), its number (`2.3`),
/// or a paragraph id.
pub fn find_heading(doc: &Document, query: &str) -> anyhow::Result<usize> {
    let q = query.trim();
    let markers = doc.list_markers();
    let heads: Vec<usize> = (0..doc.len())
        .filter(|&i| {
            doc.paragraph(i).cell.is_none() && heading_level(doc, doc.paragraph(i)).is_some()
        })
        .collect();
    let exact: Vec<usize> = heads
        .iter()
        .copied()
        .filter(|&i| {
            shown(doc, i).trim().eq_ignore_ascii_case(q)
                || markers.get(i).cloned().flatten().is_some_and(|m| {
                    m.trim().trim_end_matches(['.', ')']) == q.trim_end_matches('.')
                })
        })
        .collect();
    if let [one] = exact.as_slice() {
        return Ok(*one);
    }
    if exact.len() > 1 {
        anyhow::bail!(
            "{} headings are \"{q}\" (ids {:?}): give the id",
            exact.len(),
            exact
        );
    }
    let lower = q.to_lowercase();
    let part: Vec<usize> = heads
        .iter()
        .copied()
        .filter(|&i| shown(doc, i).to_lowercase().contains(&lower))
        .collect();
    match part.as_slice() {
        [one] => Ok(*one),
        [] => anyhow::bail!(
            "no heading \"{q}\"; headings: {}",
            heads
                .iter()
                .take(40)
                .map(|&i| format!("\"{}\"", shown(doc, i).trim()))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        many => anyhow::bail!(
            "{} headings contain \"{q}\" (ids {:?}): be more exact",
            many.len(),
            many
        ),
    }
}

/// The start of paragraph `i` and the end of paragraph `j`.
pub fn span(doc: &Document, i: usize, j: usize) -> docs_engine::Selection {
    docs_engine::Selection {
        anchor: Position::new(i, 0),
        head: Position::new(j, doc.paragraph(j).text.len()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    pub(crate) fn sample() -> Document {
        let built = crate::agent::docs_author::build(
            &json!({
                "properties": {"title": "T"},
                "blocks": [
                    {"h1": "Intro"}, "Body **bold** text.",
                    {"h2": "Detail"}, {"type": "table", "header": ["A", "B"], "rows": [["1", "2"]], "caption": "Numbers"},
                    {"h1": "Next"}, {"text": "Hand-made", "bold": true, "size": 16},
                ]
            }),
            None,
        )
        .unwrap();
        Document::from_read(docs_engine::docx::read(built.bytes).unwrap(), None)
    }

    #[std::prelude::v1::test]
    fn outlines_and_lists_text() {
        let doc = sample();
        let o = outline(&doc);
        assert_eq!(o["headings"][0]["text"], "Intro");
        assert_eq!(o["headings"][0]["number"], "1");
        assert_eq!(o["headings"][0]["children"][0]["text"], "Detail");
        assert_eq!(o["headings"][1]["number"], "2");
        assert_eq!(o["tables"][0]["rows"], 2);
        assert_eq!(o["tables"][0]["header_rows"], 1);
        assert!(o["pages"].as_u64().unwrap() >= 1);
        assert!(o["direct_formatting"]["runs"].as_u64().unwrap() >= 1);
        let t = text(&doc, 0, 3);
        assert_eq!(t["paragraphs"][0]["heading"], 1);
        assert_eq!(t["next_start"], 3);
        let intro = find_heading(&doc, "intro").unwrap();
        assert_eq!(find_heading(&doc, "1").unwrap(), intro);
        let next = find_heading(&doc, "Next").unwrap();
        assert_eq!(section_end(&doc, intro), next);
        assert!(find_heading(&doc, "Nope").is_err());
    }
}
