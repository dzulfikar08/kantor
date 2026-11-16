//! Tracked field code edits, as Word records them.
//!
//! With Track Changes on, changing a field's code (in its code box, or
//! typed with field codes shown) keeps the old code's runs in a `w:del`
//! (`w:delInstrText`) and puts the new one in a `w:ins`, inside the field's
//! code (between `fldChar begin` and `separate`). The field works with the
//! new code. Accepting drops the old runs; rejecting brings them back and
//! drops the new ones. The whole edit is one change in the list (shown as
//! the old code deleted and the new one inserted).

use std::sync::Arc;

use crate::object::FieldMark;

/// `<w:name …>…</w:name>` blocks in `code`, as byte ranges (the open tag
/// end and the close tag start too).
fn blocks(code: &str, name: &str) -> Vec<(usize, usize, usize, usize)> {
    let open = format!("<w:{name} ");
    let open_bare = format!("<w:{name}>");
    let close = format!("</w:{name}>");
    let mut out = Vec::new();
    let mut from = 0;
    loop {
        let a = [code[from..].find(&open), code[from..].find(&open_bare)]
            .into_iter()
            .flatten()
            .min();
        let Some(a) = a.map(|a| a + from) else { break };
        let Some(tag_end) = code[a..].find('>').map(|i| a + i + 1) else {
            break;
        };
        if code[..tag_end].ends_with("/>") {
            from = tag_end;
            continue;
        }
        let Some(c) = code[tag_end..].find(&close).map(|i| tag_end + i) else {
            break;
        };
        out.push((a, tag_end, c, c + close.len()));
        from = c + close.len();
    }
    out
}

/// Whether a field's code holds a tracked edit (`w:ins` / `w:del` runs).
pub(crate) fn is_tracked(code: &str) -> bool {
    !blocks(code, "ins").is_empty() || !blocks(code, "del").is_empty()
}

/// `code` with its `keep` blocks unwrapped and its `drop` blocks removed.
fn variant(code: &str, keep: &str, drop: &str) -> String {
    let mut out = code.to_owned();
    let mut cuts: Vec<(usize, usize, Option<String>)> = Vec::new();
    for (a, _, _, e) in blocks(code, drop) {
        cuts.push((a, e, None));
    }
    for (a, t, c, e) in blocks(code, keep) {
        let mut inner = code[t..c].to_owned();
        if keep == "del" {
            inner = undelete(&inner);
        }
        cuts.push((a, e, Some(inner)));
    }
    cuts.sort_by_key(|c| std::cmp::Reverse(c.0));
    for (a, e, with) in cuts {
        out.replace_range(a..e, with.as_deref().unwrap_or(""));
    }
    out
}

/// Runs written as deleted (`w:delInstrText`, `w:delText`).
fn delete_runs(xml: &str) -> String {
    xml.replace("<w:instrText", "<w:delInstrText")
        .replace("</w:instrText>", "</w:delInstrText>")
        .replace("<w:t>", "<w:delText>")
        .replace("<w:t ", "<w:delText ")
        .replace("</w:t>", "</w:delText>")
}

fn undelete(xml: &str) -> String {
    xml.replace("<w:delInstrText", "<w:instrText")
        .replace("</w:delInstrText>", "</w:instrText>")
        .replace("<w:delText", "<w:t")
        .replace("</w:delText>", "</w:t>")
}

/// A field start with code `code` (its instruction worked out from it).
pub(crate) fn with_code(code: String, sep: Option<Arc<str>>) -> FieldMark {
    let probe = FieldMark::Begin {
        instr: String::new(),
        code: code.clone().into(),
        sep: None,
        simple: false,
    };
    let shown = probe.shown_code().unwrap_or_default();
    let instr = match FieldMark::begin_nested(&format!(" {shown} ")) {
        FieldMark::Begin { instr, .. } => instr,
        FieldMark::End { .. } => String::new(),
    };
    FieldMark::Begin {
        instr,
        code: code.into(),
        sep,
        simple: false,
    }
}

/// The field start `mark` with its tracked code edit accepted (the new
/// code kept) or rejected (the old one back). `None` when it has none.
pub(crate) fn resolve(mark: &FieldMark, accept: bool) -> Option<FieldMark> {
    let FieldMark::Begin {
        code,
        sep,
        simple: false,
        ..
    } = mark
    else {
        return None;
    };
    if !is_tracked(code) {
        return None;
    }
    let code = if accept {
        variant(code, "ins", "del")
    } else {
        variant(code, "del", "ins")
    };
    Some(with_code(code, sep.clone()))
}

/// A tracked code edit's old and new code (as the codes view shows them),
/// author and date.
pub(crate) fn change(mark: &FieldMark) -> Option<(String, String, String, String)> {
    let FieldMark::Begin { code, .. } = mark else {
        return None;
    };
    if !is_tracked(code) {
        return None;
    }
    let old = resolve(mark, false)?.shown_code().unwrap_or_default();
    let new = mark.shown_code().unwrap_or_default();
    let tag = blocks(code, "ins")
        .first()
        .or(blocks(code, "del").first())
        .map(|&(a, t, _, _)| code[a..t].to_owned())
        .unwrap_or_default();
    let (author, date) = crate::fmt_track::who_when(&tag);
    Some((old, new, author, date))
}

/// The end of a code's first run (`fldChar begin`), when it holds nothing
/// else.
fn head_end(code: &str) -> Option<usize> {
    let e = code.find("</w:r>")? + "</w:r>".len();
    let head = &code[..e];
    (head.contains("fldCharType=\"begin\"") && !head.contains("instrText")).then_some(e)
}

/// The field start `new` (the code of `old` changed) with the change
/// tracked: the old code's runs deleted and the new ones inserted, tagged
/// with ids from `ins_tag(…)`/`del_tag(…)` (`w:id`, author, date
/// attributes). A code changed back to what it was before the tracked
/// edit is the old field again. `None` when the code can't be split into
/// runs (`new` goes in untracked then).
pub(crate) fn track(old: &FieldMark, new: &FieldMark, tags: (&str, &str)) -> Option<FieldMark> {
    let (
        FieldMark::Begin {
            code: oc,
            simple: false,
            ..
        },
        FieldMark::Begin { code: nc, sep, .. },
    ) = (old, new)
    else {
        return None;
    };
    let original = if is_tracked(oc) {
        resolve(old, false)?
    } else {
        old.clone()
    };
    let FieldMark::Begin { code: orig, .. } = &original else {
        return None;
    };
    if original.shown_code() == new.shown_code() {
        let FieldMark::Begin { instr, code, .. } = original else {
            return None;
        };
        return Some(FieldMark::Begin {
            instr,
            code,
            sep: sep.clone(),
            simple: false,
        });
    }
    let h = head_end(orig)?;
    let hn = head_end(nc)?;
    let mut code = orig[..h].to_owned();
    let (old_rest, new_rest) = (&orig[h..], &nc[hn..]);
    let (del_tag, ins_tag) = tags;
    if !old_rest.is_empty() {
        code.push_str(&format!(
            "<w:del {del_tag}>{}</w:del>",
            delete_runs(old_rest)
        ));
    }
    if !new_rest.is_empty() {
        code.push_str(&format!("<w:ins {ins_tag}>{new_rest}</w:ins>"));
    }
    Some(with_code(code, sep.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tracked_code_resolves_both_ways() {
        let old = FieldMark::begin(" PAGE ");
        let new = FieldMark::begin(" NUMPAGES ");
        let t = track(
            &old,
            &new,
            (
                "w:id=\"1\" w:author=\"A\" w:date=\"2026-01-01T00:00:00Z\"",
                "w:id=\"2\" w:author=\"A\" w:date=\"2026-01-01T00:00:00Z\"",
            ),
        )
        .unwrap();
        assert_eq!(t.instr().unwrap().trim(), "NUMPAGES");
        assert_eq!(t.shown_code().unwrap(), "NUMPAGES");
        let (o, n, a, _) = change(&t).unwrap();
        assert_eq!(
            (o.as_str(), n.as_str(), a.as_str()),
            ("PAGE", "NUMPAGES", "A")
        );
        assert_eq!(resolve(&t, false).unwrap().shown_code().unwrap(), "PAGE");
        assert_eq!(resolve(&t, false).unwrap().instr().unwrap().trim(), "PAGE");
        assert!(!is_tracked(match &resolve(&t, true).unwrap() {
            FieldMark::Begin { code, .. } => code,
            _ => unreachable!(),
        }));
        // Changed again: still one edit from the first code; changed back:
        // untracked.
        let again = track(
            &t,
            &FieldMark::begin(" DATE "),
            ("w:id=\"3\"", "w:id=\"4\""),
        )
        .unwrap();
        assert_eq!(change(&again).unwrap().0, "PAGE");
        let back = track(&again, &FieldMark::begin(" PAGE "), ("", "")).unwrap();
        assert!(change(&back).is_none());
    }

    #[test]
    fn nested_fields_in_the_old_code_stay_out_of_the_shown_code() {
        let old = FieldMark::begin_nested(" IF { MERGEFIELD A } = 1 \"x\" ");
        let new = FieldMark::begin(" PAGE ");
        let t = track(&old, &new, ("w:id=\"1\"", "w:id=\"2\"")).unwrap();
        assert_eq!(t.shown_code().unwrap(), "PAGE");
        assert_eq!(
            resolve(&t, false).unwrap().shown_code().unwrap(),
            "IF { MERGEFIELD A } = 1 \"x\""
        );
    }
}
