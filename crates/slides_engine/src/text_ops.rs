//! Editing a text body's paragraphs: positions are `(paragraph, byte
//! offset)` ([`docs_engine::Position`]), the same as in Docs.

use docs_engine::model::{next_grapheme, prev_grapheme};
use docs_engine::{Position, Selection};

use crate::model::{RunProps, TextPara};

pub fn clamp(paras: &[TextPara], pos: Position) -> Position {
    let para = pos.para.min(paras.len().saturating_sub(1));
    let text = &paras[para].text;
    let mut offset = pos.offset.min(text.len());
    while !text.is_char_boundary(offset) {
        offset -= 1;
    }
    Position::new(para, offset)
}

pub fn end(paras: &[TextPara]) -> Position {
    let last = paras.len().saturating_sub(1);
    Position::new(last, paras.get(last).map_or(0, |p| p.text.len()))
}

/// Delete `start..end` (document order); returns `start`.
pub fn delete_range(paras: &mut Vec<TextPara>, start: Position, end: Position) -> Position {
    let (start, end) = (clamp(paras, start), clamp(paras, end));
    if start == end {
        return start;
    }
    if start.para == end.para {
        paras[start.para].delete(start.offset..end.offset);
        return start;
    }
    let mut tail = paras[end.para].clone();
    tail.delete(0..end.offset);
    let len = paras[start.para].text.len();
    paras[start.para].delete(start.offset..len);
    paras.drain(start.para + 1..=end.para);
    paras[start.para].append(tail);
    start
}

/// Replace the selection with `text` (`\n` starts a new paragraph).
pub fn insert_text(
    paras: &mut Vec<TextPara>,
    sel: Selection,
    text: &str,
    props: Option<RunProps>,
) -> Selection {
    let (a, b) = sel.ordered();
    let start = delete_range(paras, a, b);
    let props = props.unwrap_or_else(|| paras[start.para].props_at(start.offset));
    let mut pos = start;
    let text = text.replace("\r\n", "\n").replace('\r', "\n");
    for (i, line) in text.split('\n').enumerate() {
        if i > 0 {
            let right = paras[pos.para].split_off(pos.offset);
            paras.insert(pos.para + 1, right);
            pos = Position::new(pos.para + 1, 0);
        }
        paras[pos.para].insert(pos.offset, line, props.clone());
        pos.offset += line.len();
    }
    Selection::caret(pos)
}

pub fn backspace(paras: &mut Vec<TextPara>, sel: Selection) -> Selection {
    if !sel.is_empty() {
        let (a, b) = sel.ordered();
        return Selection::caret(delete_range(paras, a, b));
    }
    let pos = clamp(paras, sel.head);
    if pos.offset > 0 {
        let prev = prev_grapheme(&paras[pos.para].text, pos.offset);
        return Selection::caret(delete_range(paras, Position::new(pos.para, prev), pos));
    }
    if pos.para == 0 {
        return Selection::caret(pos);
    }
    let start = Position::new(pos.para - 1, paras[pos.para - 1].text.len());
    Selection::caret(delete_range(paras, start, pos))
}

pub fn delete_forward(paras: &mut Vec<TextPara>, sel: Selection) -> Selection {
    if !sel.is_empty() {
        let (a, b) = sel.ordered();
        return Selection::caret(delete_range(paras, a, b));
    }
    let pos = clamp(paras, sel.head);
    let text = &paras[pos.para].text;
    let to = if pos.offset < text.len() {
        Position::new(pos.para, next_grapheme(text, pos.offset))
    } else if pos.para + 1 < paras.len() {
        Position::new(pos.para + 1, 0)
    } else {
        return Selection::caret(pos);
    };
    Selection::caret(delete_range(paras, pos, to))
}

/// Apply `f` to the formatting of every run in the selection.
pub fn format(paras: &mut [TextPara], sel: Selection, f: &impl Fn(&mut RunProps)) {
    let (a, b) = sel.ordered();
    for i in a.para..=b.para.min(paras.len().saturating_sub(1)) {
        let len = paras[i].text.len();
        let from = if i == a.para { a.offset.min(len) } else { 0 };
        let to = if i == b.para { b.offset.min(len) } else { len };
        if from < to || len == 0 {
            paras[i].format(from..to, f);
        }
    }
}

/// The position of character `c` of the paragraphs' text joined with
/// one separator character between paragraphs.
fn char_pos(paras: &[TextPara], mut c: usize) -> Position {
    for (i, p) in paras.iter().enumerate() {
        let n = p.text.chars().count();
        if c <= n || i + 1 == paras.len() {
            let off = p
                .text
                .char_indices()
                .nth(c)
                .map_or(p.text.len(), |(o, _)| o);
            return Position::new(i, off);
        }
        c -= n + 1;
    }
    Position::default()
}

/// Make the paragraphs' text `new` (paragraphs and line breaks as `\n`)
/// by replacing only what differs: the common start and end keep their
/// runs and paragraph formatting.
pub fn replace_changed(paras: &mut Vec<TextPara>, new: &str) {
    let old: Vec<char> = paras
        .iter()
        .map(|p| p.text.replace(docs_engine::LINE_BREAK, "\n"))
        .collect::<Vec<_>>()
        .join("\n")
        .chars()
        .collect();
    let new: Vec<char> = new.chars().collect();
    let pre = old.iter().zip(&new).take_while(|(a, b)| a == b).count();
    let max_suf = old.len().min(new.len()) - pre;
    let suf = old
        .iter()
        .rev()
        .zip(new.iter().rev())
        .take(max_suf)
        .take_while(|(a, b)| a == b)
        .count();
    let (a, b) = (char_pos(paras, pre), char_pos(paras, old.len() - suf));
    let middle: String = new[pre..new.len() - suf].iter().collect();
    insert_text(paras, Selection { anchor: a, head: b }, &middle, None);
}

pub fn text_in(paras: &[TextPara], start: Position, end: Position) -> String {
    let mut out = String::new();
    let last = end.para.min(paras.len().saturating_sub(1));
    for (i, p) in paras.iter().enumerate().take(last + 1).skip(start.para) {
        let t = &p.text;
        let from = if i == start.para {
            start.offset.min(t.len())
        } else {
            0
        };
        let to = if i == end.para {
            end.offset.min(t.len())
        } else {
            t.len()
        };
        if i > start.para {
            out.push('\n');
        }
        out.push_str(&t[from..to]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paras(t: &[&str]) -> Vec<TextPara> {
        t.iter().map(|s| TextPara::new(*s)).collect()
    }

    #[test]
    fn insert_and_delete_across_paragraphs() {
        let mut p = paras(&["hello", "world"]);
        let s = insert_text(
            &mut p,
            Selection::caret(Position::new(0, 5)),
            " big\nnew",
            None,
        );
        assert_eq!(
            p.iter().map(|p| p.text.as_str()).collect::<Vec<_>>(),
            ["hello big", "new", "world"]
        );
        assert_eq!(s.head, Position::new(1, 3));
        let s = backspace(&mut p, Selection::caret(Position::new(1, 0)));
        assert_eq!(p[0].text, "hello bignew");
        assert_eq!(s.head, Position::new(0, 9));
        let s = delete_forward(&mut p, Selection::caret(Position::new(0, 12)));
        assert_eq!(p[0].text, "hello bignewworld");
        assert_eq!(s.head.offset, 12);
        assert_eq!(
            text_in(&p, Position::new(0, 0), Position::new(0, 5)),
            "hello"
        );
    }

    #[test]
    fn replace_changed_keeps_formatting_outside_the_edit() {
        let mut p = paras(&["one", "two"]);
        p[1].format(0..3, &|r| r.bold = Some(true));
        p[0].props.level = 2;
        replace_changed(&mut p, "one!\ntwo");
        replace_changed(&mut p, "one!\ntwo\nthree");
        assert_eq!(
            p.iter().map(|p| p.text.as_str()).collect::<Vec<_>>(),
            ["one!", "two", "three"]
        );
        assert_eq!(p[0].props.level, 2);
        assert_eq!(p[1].runs[0].props.bold, Some(true));
        replace_changed(&mut p, "two");
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].text, "two");
        replace_changed(&mut p, "");
        assert_eq!(p[0].text, "");
        // Multibyte text: positions are characters, not bytes.
        let mut p = paras(&["héllo wörld"]);
        replace_changed(&mut p, "héllo, wörld");
        assert_eq!(p[0].text, "héllo, wörld");
    }
}
