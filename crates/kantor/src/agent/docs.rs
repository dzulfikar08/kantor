//! Docs tools on a [`Document`]. Paragraph indices count from 0; offsets
//! in answers are in characters.

use anyhow::{Result, bail};
use docs_engine::agent::NewParagraph;
use docs_engine::{Document, ListKind, OBJECT, Selection};
use kantor_core::find::{FindOptions, find_all};
use serde_json::{Value, json};

use super::{char_offset, check_len, opt_bool, opt_count, opt_int, opt_str, req_str};

const LIST_DEFAULT: u64 = 300;
const LIST_MAX: u64 = 2_000;
/// Paragraphs one call may insert.
const MAX_INSERT: usize = 2_000;

/// Paragraph text for an agent: objects as `[object]`, bookmarks dropped.
fn shown_text(doc: &Document, i: usize) -> String {
    let p = doc.paragraph(i);
    let mut out = String::with_capacity(p.text.len());
    for (offset, c) in p.text.char_indices() {
        if c != OBJECT {
            out.push(c);
        } else if !p.object_at(offset).is_some_and(|o| o.is_marker()) {
            out.push_str("[object]");
        }
    }
    out
}

fn list_name(kind: ListKind) -> &'static str {
    match kind {
        ListKind::Bullet => "bullet",
        ListKind::Numbered => "number",
    }
}

pub fn paragraphs(doc: &Document, args: &Value) -> Result<Value> {
    let start = opt_count(args, "start", 0)?.unwrap_or(0) as usize;
    let limit = opt_count(args, "limit", 1)?
        .unwrap_or(LIST_DEFAULT)
        .min(LIST_MAX) as usize;
    let end = (start + limit).min(doc.len());
    let paras: Vec<Value> = (start.min(end)..end)
        .map(|i| {
            let p = doc.paragraph(i);
            let style = p
                .props
                .style
                .as_deref()
                .map_or_else(|| "Normal".to_owned(), |s| doc.styles.display_name(s));
            let mut v = json!({"index": i, "text": shown_text(doc, i), "style": style});
            if let Some(kind) = doc.list_kind(i) {
                let level = doc.styles.effective_num(p).map_or(0, |n| n.level);
                v["list"] = json!({"kind": list_name(kind), "level": level});
            }
            if p.cell.is_some() {
                v["in_table"] = json!(true);
            }
            v
        })
        .collect();
    let mut out = json!({
        "total": doc.len(),
        "paragraphs": paras,
        "styles": doc.paragraph_style_names(),
    });
    if end < doc.len() {
        out["next_start"] = json!(end);
    }
    Ok(out)
}

pub fn selection(doc: &Document, sel: Selection) -> Value {
    let (start, end) = sel.ordered();
    let pos = |p: docs_engine::Position| json!({"paragraph": p.para, "offset": char_offset(&doc.paragraph(p.para).text, p.offset)});
    let text: String = doc.text_in(start, end).chars().take(10_000).collect();
    json!({"start": pos(start), "end": pos(end), "text": text})
}

/// Where `docs_insert_paragraphs` inserts (`None`: at the start) and what.
pub fn plan_insert(doc: &Document, args: &Value) -> Result<(Option<usize>, Vec<NewParagraph>)> {
    let after = match opt_int(args, "after_index")? {
        None => Some(doc.len() - 1),
        Some(-1) => None,
        Some(i) if i >= 0 => Some(i as usize),
        Some(_) => bail!("`after_index` must be -1 or more"),
    };
    let Some(items) = args.get("paragraphs").and_then(Value::as_array) else {
        bail!("`paragraphs` must be an array of {{text, style?, list?, level?}}");
    };
    if items.is_empty() || items.len() > MAX_INSERT {
        bail!("give between 1 and {MAX_INSERT} paragraphs");
    }
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        let item = match item {
            Value::String(s) => &json!({"text": s}),
            other => other,
        };
        let text = req_str(item, "text")?;
        check_len("a paragraph", text)?;
        let style = match opt_str(item, "style")? {
            Some(name) => doc.find_style(name).map_err(anyhow::Error::msg)?,
            None => None,
        };
        let list = match opt_str(item, "list")? {
            None => None,
            Some("bullet") => Some(ListKind::Bullet),
            Some("number") => Some(ListKind::Numbered),
            Some(other) => bail!("`list` must be bullet or number, not \"{other}\""),
        };
        let level = opt_count(item, "level", 0)?.unwrap_or(0).min(8) as u8;
        out.push(NewParagraph {
            text: text.to_owned(),
            style,
            list,
            level,
        });
    }
    Ok((after, out))
}

/// Replace every match; returns the count and the selection to show.
pub fn find_replace(
    doc: &mut Document,
    sel: Selection,
    args: &Value,
) -> Result<(usize, Selection)> {
    let find = req_str(args, "find")?;
    let with = req_str(args, "replace")?;
    if find.is_empty() {
        bail!("`find` is empty");
    }
    check_len("`replace`", with)?;
    let opts = FindOptions {
        match_case: opt_bool(args, "match_case")?.unwrap_or(false),
        whole_word: opt_bool(args, "whole_word")?.unwrap_or(false),
    };
    let ranges: Vec<_> = doc
        .paragraphs()
        .iter()
        .enumerate()
        .flat_map(|(i, p)| {
            find_all(&p.text, find, opts)
                .into_iter()
                .map(move |r| (i, r))
        })
        .collect();
    if ranges.is_empty() {
        return Ok((0, sel));
    }
    let after = doc.replace_ranges(&ranges, with, sel);
    Ok((ranges.len(), after))
}

#[cfg(test)]
mod tests {
    use super::*;
    use docs_engine::{Paragraph, Position};

    const SEL: Selection = Selection::caret(Position::new(0, 0));

    #[std::prelude::v1::test]
    fn lists_and_inserts_paragraphs() {
        let mut doc = Document::from_paragraphs(vec![Paragraph::new("Intro")]);
        let (after, items) = plan_insert(
            &doc,
            &json!({"paragraphs": [
                {"text": "Goals", "style": "Heading 1"},
                {"text": "Ship", "list": "bullet"},
                "Plain"
            ]}),
        )
        .unwrap();
        assert_eq!(after, Some(0));
        doc.insert_paragraphs(after, &items, SEL).unwrap();
        let out = paragraphs(&doc, &json!({})).unwrap();
        assert_eq!(out["total"], 4);
        assert_eq!(out["paragraphs"][1]["style"], "heading 1");
        assert_eq!(out["paragraphs"][2]["list"]["kind"], "bullet");
        assert!(out["paragraphs"][3].get("list").is_none());
        let page = paragraphs(&doc, &json!({"start": 1, "limit": 2})).unwrap();
        assert_eq!(page["paragraphs"][0]["index"], 1);
        assert_eq!(page["next_start"], 3);
        assert!(
            plan_insert(
                &doc,
                &json!({"paragraphs": [{"text": "x", "style": "Nope"}]})
            )
            .is_err()
        );
        assert!(plan_insert(&doc, &json!({"paragraphs": []})).is_err());
        assert_eq!(
            plan_insert(&doc, &json!({"after_index": -1, "paragraphs": ["x"]}))
                .unwrap()
                .0,
            None
        );
    }

    #[std::prelude::v1::test]
    fn finds_and_replaces_in_one_step() {
        let mut doc = Document::from_paragraphs(vec![
            Paragraph::new("cat and Cat"),
            Paragraph::new("concatenate"),
        ]);
        let (n, _) = find_replace(
            &mut doc,
            SEL,
            &json!({"find": "cat", "replace": "dog", "whole_word": true}),
        )
        .unwrap();
        assert_eq!(n, 2);
        assert_eq!(doc.paragraph(0).text, "dog and dog");
        assert_eq!(doc.paragraph(1).text, "concatenate");
        doc.undo();
        assert_eq!(doc.paragraph(0).text, "cat and Cat");
        let (n, _) = find_replace(&mut doc, SEL, &json!({"find": "zzz", "replace": "y"})).unwrap();
        assert_eq!(n, 0);
        let sel = Selection {
            anchor: Position::new(0, 4),
            head: Position::new(0, 7),
        };
        assert_eq!(selection(&doc, sel)["text"], "and");
    }
}
