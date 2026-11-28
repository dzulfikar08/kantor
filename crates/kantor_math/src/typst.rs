//! Typst math source for an equation ("copy as Typst"). Kantor lays
//! equations out itself (see ADR 0008); this is an export format only.

use crate::alnum;
use crate::ast::*;
use crate::symbols;

/// Functions Typst's math mode knows by name.
const TYPST_OPS: &[&str] = &[
    "arccos", "arcsin", "arctan", "arg", "cos", "cosh", "cot", "coth", "csc", "csch", "ctg", "deg",
    "det", "dim", "exp", "gcd", "lcm", "hom", "inf", "ker", "lg", "lim", "liminf", "limsup", "ln",
    "log", "max", "min", "mod", "Pr", "sec", "sech", "sin", "sinh", "sup", "tan", "tanh", "tg",
];

/// The equation as Typst source, with `$ … $` (spaces inside for
/// display math).
pub fn write(eq: &Equation) -> String {
    let body: Vec<String> = eq.lines.iter().map(|l| write_row(l)).collect();
    let body = body.join(" \\ ");
    if eq.display {
        format!("$ {body} $")
    } else {
        format!("${body}$")
    }
}

/// Typst math for a row (no `$`).
pub fn write_row(r: &[Node]) -> String {
    let parts: Vec<String> = r.iter().map(node).filter(|s| !s.is_empty()).collect();
    parts.join(" ")
}

fn group(r: &[Node]) -> String {
    format!("({})", write_row(r))
}

fn quote(s: &str) -> String {
    let mut o = String::from("\"");
    for c in s.chars() {
        match c {
            '"' | '\\' => {
                o.push('\\');
                o.push(c);
            }
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

fn text(r: &Run) -> String {
    if r.style.normal {
        return quote(&r.text);
    }
    let mut parts: Vec<String> = Vec::new();
    let mut digits = String::new();
    let mut word = String::new();
    let plain = r.style.weight == Some(Weight::Plain) && r.style.variant == Variant::Roman;
    let flush_digits = |digits: &mut String, parts: &mut Vec<String>| {
        if !digits.is_empty() {
            parts.push(std::mem::take(digits));
        }
    };
    for c in r.text.chars() {
        let (c, _, _, _) = alnum::unstyle(c);
        if c.is_ascii_digit() || (c == '.' && !digits.is_empty()) {
            digits.push(c);
            continue;
        }
        flush_digits(&mut digits, &mut parts);
        if plain && c.is_alphabetic() {
            word.push(c);
            continue;
        }
        if !word.is_empty() {
            parts.push(plain_word(&std::mem::take(&mut word)));
        }
        if c == '#' {
            // Equation numbers have no Typst math form.
            break;
        }
        if c.is_whitespace() {
            continue;
        }
        parts.push(match c {
            '/' | '_' | '^' | '$' | '\\' | '"' | '@' | '[' | ']' | '\'' | '*' => format!("\\{c}"),
            c => c.to_string(),
        });
    }
    flush_digits(&mut digits, &mut parts);
    if !word.is_empty() {
        parts.push(plain_word(&word));
    }
    let s = parts.join(" ");
    let wrap = match (r.style.variant, r.style.weight) {
        (Variant::Roman, None | Some(Weight::Plain)) => None,
        (Variant::Roman, Some(Weight::Italic)) => Some("italic"),
        (Variant::Roman, Some(Weight::Bold)) => Some("bold"),
        (Variant::Roman, Some(Weight::BoldItalic)) => Some("bold(italic"),
        (Variant::Script, _) => Some("cal"),
        (Variant::Fraktur, _) => Some("frak"),
        (Variant::DoubleStruck, _) => Some("bb"),
        (Variant::SansSerif, _) => Some("sans"),
        (Variant::Monospace, _) => Some("mono"),
    };
    match wrap {
        Some("bold(italic") => format!("bold(italic({s}))"),
        Some(f) => format!("{f}({s})"),
        None => s,
    }
}

fn plain_word(w: &str) -> String {
    if TYPST_OPS.contains(&w) {
        w.to_owned()
    } else if w.chars().count() == 1 {
        format!("upright({w})")
    } else {
        format!("upright({})", quote(w))
    }
}

fn delim(s: &str) -> String {
    match s {
        "" => String::new(),
        "[" | "]" => format!("\\{s}"),
        s => s.to_owned(),
    }
}

fn node(n: &Node) -> String {
    match n {
        Node::Text(r) => text(r),
        Node::Frac { kind, num, den } => match kind {
            FracKind::Bar => format!("frac({}, {})", write_row(num), write_row(den)),
            FracKind::Skewed => format!(
                "frac({}, {}, style: \"skewed\")",
                write_row(num),
                write_row(den)
            ),
            FracKind::Linear => format!("{} \\/ {}", group(num), group(den)),
            FracKind::NoBar => format!("mat(delim: #none, {}; {})", write_row(num), write_row(den)),
        },
        Node::Scripts { base, sub, sup } => {
            let mut s = if base.len() == 1
                && !matches!(base[0], Node::Text(ref t) if t.text.chars().count() > 1)
            {
                write_row(base)
            } else {
                format!(
                    "attach({}",
                    if base.is_empty() {
                        "\"\"".into()
                    } else {
                        write_row(base)
                    }
                )
            };
            let attach = s.starts_with("attach(");
            if attach {
                if let Some(r) = sub {
                    s.push_str(&format!(", b: {}", write_row(r)));
                }
                if let Some(r) = sup {
                    s.push_str(&format!(", t: {}", write_row(r)));
                }
                s.push(')');
            } else {
                if let Some(r) = sub {
                    s.push_str(&format!("_{}", group(r)));
                }
                if let Some(r) = sup {
                    s.push_str(&format!("^{}", group(r)));
                }
            }
            s
        }
        Node::PreScripts { sub, sup, base } => format!(
            "attach({}, tl: {}, bl: {})",
            write_row(base),
            write_row(sup),
            write_row(sub)
        ),
        Node::Radical { degree, body } => match degree {
            Some(d) => format!("root({}, {})", write_row(d), write_row(body)),
            None => format!("sqrt({})", write_row(body)),
        },
        Node::Nary {
            op,
            under_over,
            sub,
            sup,
            body,
        } => {
            let mut s = if *under_over == symbols::is_integral(*op) {
                if *under_over {
                    format!("limits({op})")
                } else {
                    format!("scripts({op})")
                }
            } else {
                op.to_string()
            };
            if let Some(r) = sub {
                s.push_str(&format!("_{}", group(r)));
            }
            if let Some(r) = sup {
                s.push_str(&format!("^{}", group(r)));
            }
            format!("{s} {}", write_row(body))
        }
        Node::Delim {
            open,
            close,
            sep,
            items,
            grow,
        } => {
            if let [item] = items.as_slice()
                && let [inner] = item.as_slice()
            {
                match (open.as_str(), close.as_str(), inner) {
                    ("{", "", Node::EqArray { rows }) => {
                        let rows: Vec<String> = rows.iter().map(|r| write_row(r)).collect();
                        return format!("cases({})", rows.join(", "));
                    }
                    (o, c, Node::Matrix { rows }) if !o.is_empty() && !c.is_empty() => {
                        return matrix(rows, &format!("\"{o}\""));
                    }
                    _ => {}
                }
            }
            let inner: Vec<String> = items.iter().map(|i| write_row(i)).collect();
            let joined = inner.join(&format!(" mid({}) ", delim(sep)));
            let (o, c) = (delim(open), delim(close));
            if *grow {
                let o = if o.is_empty() { ".".into() } else { o };
                let c = if c.is_empty() { ".".into() } else { c };
                format!("lr({o} {joined} {c})")
            } else {
                format!("{o} {joined} {c}")
            }
        }
        Node::Matrix { rows } => matrix(rows, "#none"),
        Node::EqArray { rows } => {
            let rows: Vec<String> = rows.iter().map(|r| write_row(r)).collect();
            rows.join(" \\ ")
        }
        Node::Accent { chr, body } => {
            let name = match chr {
                '\u{0302}' => "hat",
                '\u{0303}' => "tilde",
                '\u{0305}' | '\u{0304}' => "macron",
                '\u{0307}' => "dot",
                '\u{0308}' => "dot.double",
                '\u{20D7}' => "arrow",
                '\u{20D6}' => "arrow.l",
                '\u{030C}' => "caron",
                '\u{0306}' => "breve",
                '\u{0301}' => "acute",
                '\u{0300}' => "grave",
                '\u{030A}' => "circle",
                _ => "",
            };
            if name.is_empty() {
                format!("accent({}, {chr})", write_row(body))
            } else {
                format!("{name}({})", write_row(body))
            }
        }
        Node::Bar { top, body } => format!(
            "{}({})",
            if *top { "overline" } else { "underline" },
            write_row(body)
        ),
        Node::Boxed { body } => format!("#box(stroke: 0.5pt, inset: 2pt, ${}$)", write_row(body)),
        Node::Group { body } => write_row(body),
        Node::GroupChr { chr, top, body } => {
            let name = match chr {
                '⏞' => "overbrace",
                '⏟' => "underbrace",
                '⏜' => "overparen",
                '⏝' => "underparen",
                '⎴' => "overbracket",
                '⎵' => "underbracket",
                _ => "",
            };
            if name.is_empty() {
                format!(
                    "attach(limits({}), {}: {chr})",
                    write_row(body),
                    if *top { "t" } else { "b" }
                )
            } else {
                format!("{name}({})", write_row(body))
            }
        }
        Node::Limit { upper, base, limit } => format!(
            "attach(limits({}), {}: {})",
            write_row(base),
            if *upper { "t" } else { "b" },
            write_row(limit)
        ),
        Node::Func { name, body } => {
            let n = match name.as_slice() {
                [Node::Text(t)] if !t.style.normal => {
                    let w = t.text.trim();
                    if TYPST_OPS.contains(&w) {
                        w.to_owned()
                    } else {
                        format!("op({})", quote(w))
                    }
                }
                _ => write_row(name),
            };
            format!("{n} {}", write_row(body))
        }
        Node::Phantom { body, show, .. } => {
            if *show {
                write_row(body)
            } else {
                format!("#hide(${}$)", write_row(body))
            }
        }
    }
}

fn matrix(rows: &[Vec<Row>], delim: &str) -> String {
    let rows: Vec<String> = rows
        .iter()
        .map(|r| {
            r.iter()
                .map(|c| write_row(c))
                .collect::<Vec<_>>()
                .join(", ")
        })
        .collect();
    format!("mat(delim: {delim}, {})", rows.join("; "))
}
