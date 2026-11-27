//! Markdown notes → Word, PDF and slides, through the existing builders:
//! a note becomes the docs JSON (`docs_author::build`) or a slides deck
//! (`slides_author::build`). The note is read with `notes_engine`'s parser,
//! so what converts is what the editor shows: headings, paragraphs with
//! bold / italic / strikethrough / code / links, nested lists and task
//! lists, quotes, code blocks, tables and pictures on their own line.

use std::ops::Range;
use std::path::Path;

use anyhow::{Result, anyhow};
use notes_engine::parse::{LineKind, ListMarker, Parsed, SpanKind};
use notes_engine::{Buffer, parse};
use serde_json::{Value, json};

/// The note's title: its first level-1 heading, else `fallback`.
pub fn title(parsed: &Parsed, fallback: &str) -> String {
    parsed
        .headings
        .iter()
        .find(|h| h.level == 1)
        .map(|h| h.text.trim().to_owned())
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| fallback.to_owned())
}

/// Runs (docs JSON) for the content bytes of `lines`, a soft line break
/// read as a space.
fn runs(buf: &Buffer, parsed: &Parsed, lines: Range<usize>) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::new();
    for (k, l) in lines.enumerate() {
        let info = &parsed.lines[l];
        if k > 0 {
            push_run(&mut out, " ", Style::default());
        }
        for v in &info.visible {
            let mut cuts = vec![v.start, v.end];
            for s in &info.spans {
                cuts.extend([s.range.start, s.range.end]);
            }
            cuts.retain(|c| *c >= v.start && *c <= v.end);
            cuts.sort_unstable();
            cuts.dedup();
            for w in cuts.windows(2) {
                let (a, b) = (w[0], w[1]);
                if a >= b {
                    continue;
                }
                let mut st = Style::default();
                for s in &info.spans {
                    if s.range.start <= a && b <= s.range.end {
                        match s.kind {
                            SpanKind::Strong => st.bold = true,
                            SpanKind::Emphasis => st.italic = true,
                            SpanKind::Strike => st.strike = true,
                            SpanKind::Code => st.code = true,
                            SpanKind::Link(i) => {
                                if let Some(link) = parsed.links.get(i)
                                    && link.kind == notes_engine::parse::LinkKind::Url
                                {
                                    st.link = Some(link.dest.clone());
                                }
                            }
                            _ => {}
                        }
                    }
                }
                push_run(&mut out, buf.slice(a..b), st);
            }
        }
    }
    out
}

#[derive(Clone, Default, PartialEq)]
struct Style {
    bold: bool,
    italic: bool,
    strike: bool,
    code: bool,
    link: Option<String>,
}

fn push_run(out: &mut Vec<Value>, text: &str, st: Style) {
    // Join with the previous run when it looks the same.
    if let Some(Value::Object(last)) = out.last_mut()
        && style_of(last) == st
        && let Some(Value::String(t)) = last.get_mut("text")
    {
        t.push_str(text);
        return;
    }
    let mut o = serde_json::Map::new();
    o.insert("text".into(), text.into());
    if st.bold {
        o.insert("bold".into(), true.into());
    }
    if st.italic {
        o.insert("italic".into(), true.into());
    }
    if st.strike {
        o.insert("strike".into(), true.into());
    }
    if st.code {
        o.insert("code".into(), true.into());
    }
    if let Some(l) = st.link {
        o.insert("link".into(), l.into());
    }
    out.push(Value::Object(o));
}

fn style_of(o: &serde_json::Map<String, Value>) -> Style {
    let flag = |k: &str| o.get(k).and_then(Value::as_bool).unwrap_or(false);
    Style {
        bold: flag("bold"),
        italic: flag("italic"),
        strike: flag("strike"),
        code: flag("code"),
        link: o.get("link").and_then(Value::as_str).map(str::to_owned),
    }
}

fn plain(runs: &[Value]) -> String {
    runs.iter()
        .filter_map(|r| r.get("text").and_then(Value::as_str))
        .collect()
}

/// A picture alone on line `l`: its path (relative to `dir`) and alt text.
fn picture(buf: &Buffer, parsed: &Parsed, l: usize, dir: &Path) -> Option<(String, String)> {
    let info = &parsed.lines[l];
    let lr = buf.line_range(l);
    let s = info.spans.iter().find_map(|s| match s.kind {
        SpanKind::Image(i) => Some((i, s.range.clone())),
        _ => None,
    })?;
    let before = buf.slice(info.content_start.min(lr.end)..s.1.start).trim();
    let after = buf.slice(s.1.end..lr.end).trim();
    if !before.is_empty() || !after.is_empty() {
        return None;
    }
    let dest = &parsed.images.get(s.0)?.dest;
    if dest.contains("://") {
        return None;
    }
    let decoded = percent_decode(dest);
    let p = dir.join(&decoded);
    let ext = p
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    if !matches!(ext.as_str(), "png" | "jpg" | "jpeg") || !p.is_file() {
        return None;
    }
    let alt: String = info
        .visible
        .iter()
        .filter(|v| v.start >= s.1.start && v.end <= s.1.end)
        .map(|v| buf.slice(v.clone()))
        .collect();
    Some((p.to_string_lossy().into_owned(), alt))
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 3 <= b.len()
            && let Some(v) = std::str::from_utf8(&b[i + 1..i + 3])
                .ok()
                .and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| s.to_owned())
}

/// A list being collected: items with their nesting.
struct Item {
    depth: u8,
    ordered: bool,
    runs: Vec<Value>,
    children: Vec<Item>,
}

fn item_json(it: &Item) -> Value {
    let text = Value::Array(it.runs.clone());
    if it.children.is_empty() {
        return text;
    }
    let key = if it.children[0].ordered {
        "numbers"
    } else {
        "bullets"
    };
    json!({"text": text, key: it.children.iter().map(item_json).collect::<Vec<_>>()})
}

fn list_block(items: &[Item]) -> Value {
    let ordered = items.first().is_some_and(|i| i.ordered);
    json!({
        "type": "list",
        "kind": if ordered { "number" } else { "bullet" },
        "items": items.iter().map(item_json).collect::<Vec<_>>(),
    })
}

/// Put `it` at its depth under the last item of `items`.
fn nest(items: &mut Vec<Item>, it: Item, base: u8) {
    match items.last_mut() {
        Some(last) if it.depth > base => nest(&mut last.children, it, base + 1),
        _ => items.push(it),
    }
}

/// The docs JSON for a note (`dir`: its folder, for pictures).
pub fn docs_json(text: &str, fallback_title: &str, dir: &Path) -> Value {
    let buf = Buffer::new(text);
    let parsed = parse(&buf);
    let mut blocks: Vec<Value> = Vec::new();
    let mut list: Vec<Item> = Vec::new();
    let flush_list = |list: &mut Vec<Item>, blocks: &mut Vec<Value>| {
        if !list.is_empty() {
            blocks.push(list_block(list));
            list.clear();
        }
    };
    let n = buf.line_count();
    let mut l = 0;
    while l < n {
        let info = &parsed.lines[l];
        match &info.kind {
            LineKind::Heading(level) => {
                flush_list(&mut list, &mut blocks);
                // (A setext heading's underline is skipped as markup.)
                let end = l + 1;
                blocks.push(json!({
                    "type": "heading",
                    "level": (*level).clamp(1, 4),
                    "numbered": false,
                    "text": runs(&buf, &parsed, l..end),
                }));
                l = end;
            }
            LineKind::Code { block } | LineKind::CodeFence { block, .. } => {
                flush_list(&mut list, &mut blocks);
                let b = &parsed.code_blocks[*block];
                let code: Vec<String> = b
                    .lines
                    .clone()
                    .filter(|&x| matches!(parsed.lines[x].kind, LineKind::Code { .. }))
                    .map(|x| {
                        parsed.lines[x]
                            .visible
                            .iter()
                            .map(|v| buf.slice(v.clone()))
                            .collect()
                    })
                    .collect();
                if !code.is_empty() {
                    blocks.push(json!({
                        "type": "paragraph",
                        "text": [{"text": code.join("\n"), "code": true}],
                        "keep_together": true,
                    }));
                }
                l = b.lines.end.max(l + 1);
            }
            LineKind::TableRow { table, .. } | LineKind::TableDelimiter { table } => {
                flush_list(&mut list, &mut blocks);
                let t = &parsed.tables[*table];
                let cell = |row: &notes_engine::parse::TableRow, c: &Range<usize>| -> String {
                    parsed.lines[row.line]
                        .visible
                        .iter()
                        .filter_map(|v| {
                            let (a, b) = (v.start.max(c.start), v.end.min(c.end));
                            (a < b).then(|| buf.slice(a..b))
                        })
                        .collect::<String>()
                        .trim()
                        .to_owned()
                };
                let mut rows = t.rows.iter();
                let header: Vec<String> = rows
                    .next()
                    .map(|r| r.cells.iter().map(|c| cell(r, c)).collect())
                    .unwrap_or_default();
                let body: Vec<Vec<String>> = rows
                    .map(|r| {
                        let mut v: Vec<String> = r.cells.iter().map(|c| cell(r, c)).collect();
                        v.resize(header.len().max(v.len()), String::new());
                        v
                    })
                    .collect();
                let align: Vec<&str> = t
                    .aligns
                    .iter()
                    .map(|a| match a {
                        notes_engine::parse::Align::Center => "center",
                        notes_engine::parse::Align::Right => "right",
                        _ => "left",
                    })
                    .collect();
                let mut tj = json!({"type": "table", "header": header, "rows": body});
                if t.aligns
                    .iter()
                    .any(|a| *a != notes_engine::parse::Align::None)
                {
                    tj["align"] = json!(align);
                }
                blocks.push(tj);
                l = t.lines.end.max(l + 1);
            }
            LineKind::Paragraph if info.marker != ListMarker::None || info.list_depth > 0 => {
                // A list item, or a line continuing one.
                let mut text = runs(&buf, &parsed, l..l + 1);
                match &info.marker {
                    ListMarker::Task { checked, .. } => {
                        let mark = if *checked { "☑ " } else { "☐ " };
                        text.insert(0, json!({"text": mark}));
                    }
                    ListMarker::None => {
                        // Continuation: add to the last item at this depth.
                        if let Some(last) = deepest(&mut list) {
                            last.runs.push(json!({"text": " "}));
                            last.runs.extend(text);
                        }
                        l += 1;
                        continue;
                    }
                    _ => {}
                }
                let ordered = matches!(info.marker, ListMarker::Ordered(_));
                let depth = info.list_depth.max(1);
                if depth == 1 && list.first().is_some_and(|f| f.ordered != ordered) {
                    flush_list(&mut list, &mut blocks);
                }
                nest(
                    &mut list,
                    Item {
                        depth,
                        ordered,
                        runs: text,
                        children: Vec::new(),
                    },
                    1,
                );
                l += 1;
            }
            LineKind::Blank if info.list_depth > 0 => l += 1,
            LineKind::Paragraph | LineKind::Raw => {
                flush_list(&mut list, &mut blocks);
                if let Some((path, alt)) = picture(&buf, &parsed, l, dir) {
                    blocks.push(json!({"type": "picture", "path": path, "alt": alt}));
                    l += 1;
                    continue;
                }
                // Lines of one paragraph (or one quote).
                let quote = info.quote;
                let mut end = l + 1;
                while end < n {
                    let x = &parsed.lines[end];
                    if x.kind != info.kind || x.quote != quote || x.marker != ListMarker::None {
                        break;
                    }
                    end += 1;
                }
                let text = if info.kind == LineKind::Raw {
                    vec![
                        json!({"text": (l..end).map(|x| buf.line(x)).collect::<Vec<_>>().join(" ")}),
                    ]
                } else {
                    runs(&buf, &parsed, l..end)
                };
                if quote > 0 {
                    blocks.push(json!({"type": "quote", "text": plain(&text)}));
                } else {
                    blocks.push(json!({"type": "paragraph", "text": text}));
                }
                l = end;
            }
            _ => {
                if !matches!(info.kind, LineKind::Blank) || info.list_depth == 0 {
                    flush_list(&mut list, &mut blocks);
                }
                l += 1;
            }
        }
    }
    flush_list(&mut list, &mut blocks);
    json!({
        "preset": "report",
        "markdown": false,
        "numbered_headings": false,
        "properties": {"title": title(&parsed, fallback_title)},
        "blocks": blocks,
    })
}

fn deepest(items: &mut [Item]) -> Option<&mut Item> {
    let last = items.last_mut()?;
    if last.children.is_empty() {
        return Some(last);
    }
    deepest(&mut last.children)
}

/// A slides deck for a note: a `# Title` first makes a cover; every other
/// heading (and every `---`) starts a slide; paragraphs and list items
/// are its bullets (nested items one level in).
pub fn slides_json(text: &str, fallback_title: &str) -> Value {
    let buf = Buffer::new(text);
    let parsed = parse(&buf);
    let mut slides: Vec<Value> = Vec::new();
    let mut current: Option<(String, Vec<Value>)> = None;
    let mut cover_done = false;
    let note_title = title(&parsed, fallback_title);
    let finish = |cur: &mut Option<(String, Vec<Value>)>, slides: &mut Vec<Value>| {
        if let Some((title, bullets)) = cur.take()
            && (!title.is_empty() || !bullets.is_empty())
        {
            let title = if title.is_empty() {
                note_title.clone()
            } else {
                title
            };
            let mut s = json!({"title": title});
            if !bullets.is_empty() {
                s["bullets"] = Value::Array(bullets);
            }
            slides.push(s);
        }
    };
    for l in 0..buf.line_count() {
        let info = &parsed.lines[l];
        match &info.kind {
            LineKind::Heading(level) => {
                let t = plain(&runs(&buf, &parsed, l..l + 1));
                if *level == 1 && !cover_done && slides.is_empty() && current.is_none() {
                    slides.push(json!({"kind": "cover", "title": t}));
                    cover_done = true;
                    continue;
                }
                if *level <= 2 || current.is_none() {
                    finish(&mut current, &mut slides);
                    current = Some((t, Vec::new()));
                } else if let Some((_, b)) = &mut current {
                    b.push(json!({"text": t}));
                }
            }
            LineKind::Rule => finish(&mut current, &mut slides),
            LineKind::Paragraph => {
                let t = plain(&runs(&buf, &parsed, l..l + 1));
                if t.trim().is_empty() {
                    continue;
                }
                let t = match &info.marker {
                    ListMarker::Task { checked: true, .. } => format!("☑ {t}"),
                    ListMarker::Task { .. } => format!("☐ {t}"),
                    _ => t,
                };
                let level = info.list_depth.saturating_sub(1).min(4);
                // The first paragraph under the title: the cover's subtitle.
                if current.is_none()
                    && cover_done
                    && slides.len() == 1
                    && info.list_depth == 0
                    && slides[0].get("subtitle").is_none()
                {
                    slides[0]["subtitle"] = t.into();
                    continue;
                }
                let cur = current.get_or_insert_with(|| (String::new(), Vec::new()));
                if info.marker == ListMarker::None
                    && info.list_depth > 0
                    && let Some(Value::Object(last)) = cur.1.last_mut()
                    && let Some(Value::String(s)) = last.get_mut("text")
                {
                    s.push(' ');
                    s.push_str(&t);
                    continue;
                }
                cur.1.push(if level == 0 {
                    json!({"text": t})
                } else {
                    json!({"text": t, "level": level})
                });
            }
            _ => {}
        }
    }
    finish(&mut current, &mut slides);
    if slides.is_empty() {
        slides.push(json!({"kind": "cover", "title": note_title}));
    }
    json!({"preset": "editorial", "slides": slides})
}

/// Build a `.docx` from a note.
pub fn to_docx(text: &str, fallback_title: &str, dir: &Path) -> Result<Vec<u8>> {
    let json = docs_json(text, fallback_title, dir);
    Ok(super::docs_author::build(&json, Some(dir))?.bytes)
}

/// Build a PDF from a note (through the `.docx`, laid out as Docs does).
pub fn to_pdf(text: &str, fallback_title: &str, dir: &Path) -> Result<Vec<u8>> {
    let docx = to_docx(text, fallback_title, dir)?;
    let doc = docs_engine::Document::open_recovered(docx, None, Vec::new())?;
    let mut engine = docs_pdf::Engine::new();
    let laid = docs_pdf::lay_out(&doc, &mut engine);
    docs_pdf::write(&docs_pdf::scene(&doc, &laid))
}

/// Build a `.pptx` from a note.
pub fn to_pptx(text: &str, fallback_title: &str, dir: &Path) -> Result<Vec<u8>> {
    let deck = slides_json(text, fallback_title);
    let p = super::slides_author::build(&deck, Some(dir))?;
    p.to_bytes()
}

/// Read a note file as text (UTF-8).
pub fn read_note(path: &Path) -> Result<String> {
    let bytes = std::fs::read(path)?;
    let b = Buffer::from_bytes(&bytes).map_err(|e| anyhow!("{}: {e}", path.display()))?;
    Ok(b.text().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOTE: &str = "# Plan\n\nWe ship **soon** with `code`.\n\n## Steps\n\n- one\n  - one a\n- [x] two\n\n1. first\n2. second\n\n> quoted\n\n```rust\nlet x = 1;\n```\n\n| a | b |\n|---|--:|\n| 1 | 2 |\n";

    #[test]
    fn docs_blocks_follow_the_note() {
        let j = docs_json(NOTE, "x", Path::new("."));
        assert_eq!(j["properties"]["title"], "Plan");
        let blocks = j["blocks"].as_array().unwrap();
        let types: Vec<&str> = blocks.iter().map(|b| b["type"].as_str().unwrap()).collect();
        assert_eq!(
            types,
            [
                "heading",
                "paragraph",
                "heading",
                "list",
                "list",
                "quote",
                "paragraph",
                "table"
            ]
        );
        let para = &blocks[1]["text"];
        assert_eq!(para[1], json!({"text": "soon", "bold": true}));
        assert_eq!(para[3], json!({"text": "code", "code": true}));
        let bullets = &blocks[3];
        assert_eq!(bullets["kind"], "bullet");
        assert_eq!(bullets["items"][0]["bullets"][0][0]["text"], "one a");
        assert_eq!(bullets["items"][1][0]["text"], "☑ ");
        assert_eq!(blocks[4]["kind"], "number");
        assert_eq!(blocks[6]["text"][0]["text"], "let x = 1;");
        assert_eq!(blocks[7]["header"], json!(["a", "b"]));
        assert_eq!(blocks[7]["align"], json!(["left", "right"]));
    }

    #[test]
    fn slides_split_at_headings() {
        let j = slides_json(NOTE, "x");
        let slides = j["slides"].as_array().unwrap();
        assert_eq!(slides[0]["kind"], "cover");
        assert_eq!(slides[0]["title"], "Plan");
        assert_eq!(slides[0]["subtitle"], "We ship soon with code.");
        assert_eq!(slides[1]["title"], "Steps");
        let b = slides[1]["bullets"].as_array().unwrap();
        assert_eq!(b[0]["text"], "one");
        assert_eq!(b[1]["level"], 1);
        assert_eq!(b[2]["text"], "☑ two");
    }

    #[test]
    fn notes_build_into_word_and_slides() {
        let dir = tempfile::tempdir().unwrap();
        let docx = to_docx(NOTE, "x", dir.path()).unwrap();
        assert!(docx.starts_with(b"PK"));
        let pptx = to_pptx(NOTE, "x", dir.path()).unwrap();
        assert!(pptx.starts_with(b"PK"));
    }
}
