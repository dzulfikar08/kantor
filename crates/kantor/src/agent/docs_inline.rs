//! The text of a paragraph in the docs JSON: a string (with light markdown:
//! `**bold**`, `*italic*`, `` `code` ``, `[text](url)`, `[^footnote]`,
//! `{ref:id}` / `{ref:id:page}`, `{cite:key}` / `{cite:a;b@12}`; `\`
//! escapes) or a list of runs (strings and `{"text", "bold", "link",
//! "footnote", "ref", "field", "cite", ...}`).

use anyhow::{Context as _, Result, bail};
use serde_json::{Map, Value};

use super::docs_look::{Look, RunFmt, get, object, opt_bool, opt_str, points};

/// A piece of a paragraph.
#[derive(Clone, Debug, PartialEq)]
pub enum Piece {
    Text {
        text: String,
        fmt: RunFmt,
        /// A URL, or `#name` for a place in the document.
        link: Option<String>,
    },
    /// A footnote's text (pieces without notes).
    Note(Vec<Piece>),
    /// A cross-reference to the block with this `id`; `show` is `number`,
    /// `text`, `page`, `label` or `full`.
    Ref {
        id: String,
        show: Option<String>,
        fmt: RunFmt,
    },
    /// A Word field: its instruction and the result shown until updated.
    Field {
        instr: String,
        result: String,
        fmt: RunFmt,
    },
    /// A citation of sources by key (`citations.sources`), each with its
    /// page or other locator.
    Cite {
        keys: Vec<(String, Option<String>)>,
        prefix: Option<String>,
        suffix: Option<String>,
        /// Without the author's name (`Nugroho (2021)` written by hand).
        no_author: bool,
        fmt: RunFmt,
    },
}

/// `a;b@12` (a markdown citation's keys, `@` a page): keys and locators.
pub fn cite_keys(body: &str) -> Vec<(String, Option<String>)> {
    body.split(';')
        .filter_map(|k| {
            let (key, loc) = match k.split_once('@') {
                Some((k, l)) => (
                    k.trim(),
                    Some(l.trim().to_owned()).filter(|l| !l.is_empty()),
                ),
                None => (k.trim(), None),
            };
            (!key.is_empty()).then(|| (key.to_owned(), loc))
        })
        .collect()
}

/// Keys of a run object.
pub const RUN_KEYS: &[&str] = &[
    "text",
    "bold",
    "italic",
    "underline",
    "strike",
    "caps",
    "small_caps",
    "color",
    "highlight",
    "size",
    "font",
    "style",
    "superscript",
    "subscript",
    "code",
    "link",
    "footnote",
    "ref",
    "show",
    "field",
    "result",
    "cite",
    "page",
    "prefix",
    "suffix",
    "author",
];

/// Ways a cross-reference shows its target.
pub const SHOW: &[&str] = &["number", "text", "page", "label", "full"];

/// Formatting markers open at a point of a markdown string.
#[derive(Clone, Copy, Default)]
struct Marks {
    bold: bool,
    italic: bool,
    code: bool,
}

/// Find `pat` in `chars` from `from`, skipping escaped characters.
fn find(chars: &[char], from: usize, pat: &str) -> Option<usize> {
    let p: Vec<char> = pat.chars().collect();
    let mut i = from;
    while i + p.len() <= chars.len() {
        if chars[i] == '\\' {
            i += 2;
            continue;
        }
        if chars[i..i + p.len()] == p[..] {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// Parse a markdown string into pieces. `base` is the paragraph's
/// formatting, `mono` the code font; `notes` allows `[^…]`.
pub fn markdown(s: &str, base: &RunFmt, mono: &str, notes: bool) -> Vec<Piece> {
    let chars: Vec<char> = s.chars().collect();
    let mut out: Vec<Piece> = Vec::new();
    let mut buf = String::new();
    let mut m = Marks::default();
    // Marks become the character styles Strong, Emphasis and Code (real
    // styles rather than formatting by hand).
    let fmt_of = |m: Marks| {
        let mut f = base.clone();
        let free = f.style.is_none();
        match (m.code, m.bold, m.italic) {
            (true, _, _) if free => f.style = Some("Code".into()),
            (true, _, _) => f.font = Some(mono.to_owned()),
            (false, true, false) if free => f.style = Some("Strong".into()),
            (false, false, true) if free => f.style = Some("Emphasis".into()),
            (false, true, true) if free => {
                f.style = Some("Strong".into());
                f.italic = Some(true);
            }
            _ => {}
        }
        if f.style.is_none() || !free {
            if m.bold {
                f.bold = Some(true);
            }
            if m.italic {
                f.italic = Some(true);
            }
        }
        f
    };
    let flush = |buf: &mut String, out: &mut Vec<Piece>, m: Marks| {
        if !buf.is_empty() {
            out.push(Piece::Text {
                text: std::mem::take(buf),
                fmt: fmt_of(m),
                link: None,
            });
        }
    };
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        if c == '\\' && next.is_some_and(|n| "\\*`[]{}()_".contains(n)) {
            buf.push(chars[i + 1]);
            i += 2;
            continue;
        }
        if m.code {
            if c == '`' {
                flush(&mut buf, &mut out, m);
                m.code = false;
            } else {
                buf.push(c);
            }
            i += 1;
            continue;
        }
        if c == '`' && find(&chars, i + 1, "`").is_some() {
            flush(&mut buf, &mut out, m);
            m.code = true;
            i += 1;
            continue;
        }
        if c == '*' && next == Some('*') {
            if m.bold {
                flush(&mut buf, &mut out, m);
                m.bold = false;
                i += 2;
                continue;
            }
            let opens = chars.get(i + 2).is_some_and(|n| !n.is_whitespace());
            if opens && find(&chars, i + 2, "**").is_some() {
                flush(&mut buf, &mut out, m);
                m.bold = true;
                i += 2;
                continue;
            }
        } else if c == '*' {
            if m.italic && !buf.is_empty() && !buf.ends_with(char::is_whitespace) {
                flush(&mut buf, &mut out, m);
                m.italic = false;
                i += 1;
                continue;
            }
            let opens = next.is_some_and(|n| !n.is_whitespace() && n != '*');
            if !m.italic && opens {
                // A closing `*` not part of `**`.
                let mut j = i + 1;
                let mut found = false;
                while let Some(k) = find(&chars, j, "*") {
                    if chars.get(k + 1) == Some(&'*') {
                        j = k + 2;
                        continue;
                    }
                    found = !chars[k - 1].is_whitespace();
                    break;
                }
                if found {
                    flush(&mut buf, &mut out, m);
                    m.italic = true;
                    i += 1;
                    continue;
                }
            }
        }
        if c == '['
            && next == Some('^')
            && notes
            && let Some(end) = find(&chars, i + 2, "]")
        {
            flush(&mut buf, &mut out, m);
            let inner: String = chars[i + 2..end].iter().collect();
            out.push(Piece::Note(markdown(
                inner.trim(),
                &RunFmt::default(),
                mono,
                false,
            )));
            i = end + 1;
            continue;
        }
        if c == '['
            && let Some(mid) = find(&chars, i + 1, "](")
            && let Some(end) = find(&chars, mid + 2, ")")
        {
            let label: String = chars[i + 1..mid].iter().collect();
            let url: String = chars[mid + 2..end].iter().collect();
            if !label.is_empty() && !url.trim().is_empty() && !url.contains(char::is_whitespace) {
                flush(&mut buf, &mut out, m);
                for p in markdown(&label, &fmt_of(m), mono, false) {
                    if let Piece::Text { text, fmt, .. } = p {
                        out.push(Piece::Text {
                            text,
                            fmt,
                            link: Some(url.trim().to_owned()),
                        });
                    }
                }
                i = end + 1;
                continue;
            }
        }
        if c == '{'
            && chars[i..].starts_with(&['{', 'r', 'e', 'f', ':'])
            && let Some(end) = find(&chars, i + 5, "}")
        {
            let body: String = chars[i + 5..end].iter().collect();
            let (id, show) = match body.split_once(':') {
                Some((id, show)) => (id.trim().to_owned(), Some(show.trim().to_owned())),
                None => (body.trim().to_owned(), None),
            };
            if !id.is_empty() {
                flush(&mut buf, &mut out, m);
                out.push(Piece::Ref {
                    id,
                    show,
                    fmt: fmt_of(m),
                });
                i = end + 1;
                continue;
            }
        }
        if c == '{'
            && chars[i..].starts_with(&['{', 'c', 'i', 't', 'e', ':'])
            && let Some(end) = find(&chars, i + 6, "}")
        {
            let body: String = chars[i + 6..end].iter().collect();
            let keys = cite_keys(&body);
            if !keys.is_empty() {
                flush(&mut buf, &mut out, m);
                out.push(Piece::Cite {
                    keys,
                    prefix: None,
                    suffix: None,
                    no_author: false,
                    fmt: fmt_of(m),
                });
                i = end + 1;
                continue;
            }
        }
        buf.push(c);
        i += 1;
    }
    flush(&mut buf, &mut out, m);
    out
}

/// One run object.
fn run(o: &Map<String, Value>, look: &Look, base: &RunFmt, out: &mut Vec<Piece>) -> Result<()> {
    let mut f = base.clone();
    let (r, _) = look.formats(&{
        let mut m = Map::new();
        for k in [
            "font",
            "size",
            "bold",
            "italic",
            "underline",
            "strike",
            "caps",
            "small_caps",
            "color",
            "highlight",
        ] {
            if let Some(v) = o.get(k) {
                m.insert(k.to_owned(), v.clone());
            }
        }
        m
    })?;
    f.overlay(&r);
    if let Some(s) = opt_str(o, "style")? {
        f.style = Some(look.style_id(s)?);
    }
    if opt_bool(o, "superscript")? == Some(true) {
        f.vert = Some("superscript");
    }
    if opt_bool(o, "subscript")? == Some(true) {
        f.vert = Some("subscript");
    }
    // Code is the Code character style (so overriding the style changes
    // every code run); a run that already has a style gets the mono font.
    if opt_bool(o, "code")? == Some(true) {
        if f.style.is_none() {
            f.style = Some("Code".into());
        } else {
            f.font = Some(look.font("mono"));
        }
    }
    if let Some(v) = get(o, "size") {
        points(v, "size")?;
    }
    let link = opt_str(o, "link")?.map(str::to_owned);
    if let Some(t) = opt_str(o, "text")? {
        super::check_len("a run", t)?;
        out.push(Piece::Text {
            text: t.to_owned(),
            fmt: f.clone(),
            link: link.clone(),
        });
    }
    if let Some(id) = opt_str(o, "ref")? {
        let show = opt_str(o, "show")?.map(str::to_owned);
        if let Some(s) = &show
            && !SHOW.contains(&s.as_str())
        {
            bail!("`show` must be one of {}, not \"{s}\"", SHOW.join(", "));
        }
        out.push(Piece::Ref {
            id: id.to_owned(),
            show,
            fmt: f.clone(),
        });
    }
    if let Some(instr) = opt_str(o, "field")? {
        let instr = instr.trim();
        if instr.is_empty() || instr.contains(['{', '}']) {
            bail!("`field` must be a field instruction such as \"DATE \\\\@ \\\"d MMMM yyyy\\\"\"");
        }
        out.push(Piece::Field {
            instr: instr.to_owned(),
            result: opt_str(o, "result")?.unwrap_or("").to_owned(),
            fmt: f.clone(),
        });
    }
    if let Some(c) = get(o, "cite") {
        let mut keys: Vec<(String, Option<String>)> = match c {
            Value::String(s) => cite_keys(s),
            Value::Array(a) => a
                .iter()
                .map(|k| {
                    k.as_str()
                        .map(|k| (k.trim().to_owned(), None))
                        .context("`cite` lists source keys: [\"nugroho2021\", …]")
                })
                .collect::<Result<_>>()?,
            _ => bail!("`cite` is a source key or a list of keys"),
        };
        if keys.is_empty() {
            bail!("`cite` needs a source key");
        }
        if let Some(p) = get(o, "page") {
            let p = match p {
                Value::String(s) => s.trim().to_owned(),
                Value::Number(n) => n.to_string(),
                _ => bail!("`page` is a page or page range, e.g. \"12\" or \"12-15\""),
            };
            if let Some(last) = keys.last_mut() {
                last.1 = Some(p).filter(|p| !p.is_empty());
            }
        }
        out.push(Piece::Cite {
            keys,
            prefix: opt_str(o, "prefix")?.map(str::to_owned),
            suffix: opt_str(o, "suffix")?.map(str::to_owned),
            no_author: opt_bool(o, "author")? == Some(false),
            fmt: f.clone(),
        });
    } else if ["page", "prefix", "suffix", "author"]
        .iter()
        .any(|k| o.contains_key(*k))
    {
        bail!("`page`, `prefix`, `suffix` and `author` go with `cite`");
    }
    if let Some(n) = get(o, "footnote") {
        let note = match n {
            Value::String(s) => markdown(s, &RunFmt::default(), &look.font("mono"), false),
            Value::Array(_) => {
                let mut v = Vec::new();
                pieces_into(n, look, &RunFmt::default(), false, false, &mut v)?;
                v
            }
            _ => bail!("`footnote` must be text"),
        };
        if note.iter().any(|p| matches!(p, Piece::Note(_))) {
            bail!("a footnote can't hold another footnote");
        }
        out.push(Piece::Note(note));
    }
    if link.is_some() && opt_str(o, "text")?.is_none() {
        bail!("a run with `link` needs `text`");
    }
    Ok(())
}

fn pieces_into(
    v: &Value,
    look: &Look,
    base: &RunFmt,
    md: bool,
    notes: bool,
    out: &mut Vec<Piece>,
) -> Result<()> {
    match v {
        Value::String(s) => {
            super::check_len("a paragraph", s)?;
            if md {
                out.extend(markdown(s, base, &look.font("mono"), notes));
            } else {
                out.push(Piece::Text {
                    text: s.clone(),
                    fmt: base.clone(),
                    link: None,
                });
            }
        }
        Value::Number(n) => out.push(Piece::Text {
            text: n.to_string(),
            fmt: base.clone(),
            link: None,
        }),
        Value::Array(items) => {
            for it in items {
                match it {
                    Value::Object(_) => {
                        let o = object(it, "a run", RUN_KEYS)?;
                        run(o, look, base, out)?;
                    }
                    other => pieces_into(other, look, base, md, notes, out)?,
                }
            }
        }
        Value::Object(_) => {
            let o = object(v, "a run", RUN_KEYS)?;
            run(o, look, base, out)?;
        }
        Value::Null => {}
        Value::Bool(_) => bail!("text must be a string or a list of runs"),
    }
    Ok(())
}

/// The pieces of paragraph text `v` (a string or runs).
pub fn pieces(v: &Value, look: &Look, base: &RunFmt, md: bool) -> Result<Vec<Piece>> {
    let mut out = Vec::new();
    pieces_into(v, look, base, md, true, &mut out)?;
    if out
        .iter()
        .any(|p| matches!(p, Piece::Note(n) if n.iter().any(|q| matches!(q, Piece::Note(_)))))
    {
        bail!("a footnote can't hold another footnote");
    }
    Ok(out)
}

/// The text the pieces show (references as their ids, notes left out).
pub fn plain(pieces: &[Piece]) -> String {
    pieces
        .iter()
        .map(|p| match p {
            Piece::Text { text, .. } => text.clone(),
            Piece::Field { result, .. } => result.clone(),
            Piece::Ref { id, .. } => format!("[{id}]"),
            Piece::Cite { keys, .. } => format!(
                "({})",
                keys.iter()
                    .map(|k| k.0.as_str())
                    .collect::<Vec<_>>()
                    .join("; ")
            ),
            Piece::Note(_) => String::new(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn texts(p: &[Piece]) -> Vec<(String, bool, bool)> {
        p.iter()
            .filter_map(|p| match p {
                Piece::Text { text, fmt, .. } => Some((
                    text.clone(),
                    fmt.bold == Some(true) || fmt.style.as_deref() == Some("Strong"),
                    fmt.italic == Some(true) || fmt.style.as_deref() == Some("Emphasis"),
                )),
                _ => None,
            })
            .collect()
    }

    #[std::prelude::v1::test]
    fn markdown_marks() {
        let b = RunFmt::default();
        let p = markdown("a **b** *c* d", &b, "Mono", true);
        assert_eq!(
            texts(&p),
            vec![
                ("a ".into(), false, false),
                ("b".into(), true, false),
                (" ".into(), false, false),
                ("c".into(), false, true),
                (" d".into(), false, false)
            ]
        );
        // Lone stars, maths and escapes stay text.
        assert_eq!(
            plain(&markdown("5 * 3 = 15, a*", &b, "M", true)),
            "5 * 3 = 15, a*"
        );
        assert_eq!(
            plain(&markdown("\\*not\\* italic", &b, "M", true)),
            "*not* italic"
        );
        let p = markdown(
            "See [the site](https://example.com) and `x_y`.",
            &b,
            "Mono",
            true,
        );
        assert!(p.iter().any(|p| matches!(p, Piece::Text { link: Some(l), text, .. } if l == "https://example.com" && text == "the site")));
        assert!(p.iter().any(|p| matches!(p, Piece::Text { fmt, text, .. } if fmt.style.as_deref() == Some("Code") && text == "x_y")));
        let p = markdown(
            "Growth[^Source: annual report.] is up, see {ref:fig-1} on {ref:fig-1:page}.",
            &b,
            "M",
            true,
        );
        assert!(matches!(&p[1], Piece::Note(n) if plain(n) == "Source: annual report."));
        assert!(
            p.iter()
                .any(|p| matches!(p, Piece::Ref { id, show: None, .. } if id == "fig-1"))
        );
        assert!(
            p.iter()
                .any(|p| matches!(p, Piece::Ref { show: Some(s), .. } if s == "page"))
        );
        // No notes inside notes.
        assert_eq!(plain(&markdown("[^x]", &b, "M", false)), "[^x]");
        assert_eq!(
            plain(&markdown("[TBD] and [name]", &b, "M", true)),
            "[TBD] and [name]"
        );
    }

    #[std::prelude::v1::test]
    fn run_objects() {
        let look =
            Look::read(&super::super::docs_look::preset("report").unwrap(), "en-US").unwrap();
        let p = pieces(
            &json!(["Plain ", {"text": "bold", "bold": true, "color": "accent"}, {"text": "x", "footnote": "A *note*."}, {"ref": "t1", "show": "page"}, {"field": "DATE", "result": "today"}]),
            &look,
            &RunFmt::default(),
            true,
        )
        .unwrap();
        assert_eq!(p.len(), 6);
        assert!(matches!(&p[1], Piece::Text { fmt, .. } if fmt.color.as_deref() == Some("2F4B7C")));
        assert!(matches!(&p[3], Piece::Note(n) if n.len() == 3));
        assert!(pieces(&json!([{"txt": "x"}]), &look, &RunFmt::default(), true).is_err());
        assert!(
            pieces(
                &json!([{"ref": "a", "show": "nope"}]),
                &look,
                &RunFmt::default(),
                true
            )
            .is_err()
        );
        assert!(
            pieces(
                &json!([{"link": "https://x"}]),
                &look,
                &RunFmt::default(),
                true
            )
            .is_err()
        );
    }

    #[std::prelude::v1::test]
    fn citations_in_markdown() {
        assert_eq!(
            cite_keys(" a@12 ; b ;"),
            vec![
                ("a".to_owned(), Some("12".to_owned())),
                ("b".to_owned(), None)
            ]
        );
        let b = RunFmt::default();
        let p = markdown("Hasil {cite:nugroho2021@118;santoso2019}.", &b, "M", true);
        assert!(p.iter().any(|p| matches!(p, Piece::Cite { keys, .. } if keys.len() == 2 && keys[0].1.as_deref() == Some("118"))));
        assert_eq!(plain(&p), "Hasil (nugroho2021; santoso2019).");
        // Not a citation: empty, escaped.
        assert_eq!(plain(&markdown("{cite:}", &b, "M", true)), "{cite:}");
        assert_eq!(plain(&markdown("\\{cite:x}", &b, "M", true)), "{cite:x}");
    }
}
