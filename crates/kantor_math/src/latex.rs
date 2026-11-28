//! LaTeX math to and from the model: the commands people type for
//! theses and papers (amsmath's environments included), not TeX's macro
//! language.

use crate::alnum;
use crate::ast::*;
use crate::symbols::{self, Class};

/// A LaTeX error: what's wrong and the character offset where.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error {
    pub message: String,
    pub offset: usize,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} (at {})", self.message, self.offset)
    }
}

impl std::error::Error for Error {}

type Res<T> = Result<T, Error>;

/// Parses LaTeX math (no surrounding `$`; they're tolerated) into an
/// equation. `display` picks display or inline math.
pub fn parse(src: &str, display: bool) -> Res<Equation> {
    let mut src = src.trim();
    let mut display = display;
    for (open, close, disp) in [
        ("$$", "$$", true),
        ("\\[", "\\]", true),
        ("\\(", "\\)", false),
        ("$", "$", false),
    ] {
        if src.len() >= open.len() + close.len() && src.starts_with(open) && src.ends_with(close) {
            src = &src[open.len()..src.len() - close.len()];
            display = disp;
            break;
        }
    }
    let mut p = Parser {
        s: src.chars().collect(),
        i: 0,
    };
    let row = p.top()?;
    Ok(Equation {
        display,
        justify: Justify::CenterGroup,
        lines: vec![row],
    })
}

/// Parses LaTeX into a row (no display/inline choice).
pub fn parse_row(src: &str) -> Res<Row> {
    Ok(parse(src, false)?
        .lines
        .into_iter()
        .next()
        .unwrap_or_default())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Close {
    /// End of input.
    End,
    /// `}`.
    Brace,
    /// `]` (an optional argument).
    Bracket,
    /// `\right`.
    Right,
    /// Inside an environment: `&`, `\\` and `\end` end a cell.
    Env,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Hit {
    End,
    Amp,
    NewRow,
    /// `\right` or `\middle` with the delimiter that follows.
    Right(String),
    Middle(String),
    /// `\end{name}`.
    EndEnv(String),
    /// A nary operand or function argument stopped before this.
    Stop,
}

struct Parser {
    s: Vec<char>,
    i: usize,
}

const OPENS: &[(&str, &str)] = &[("(", ")"), ("[", "]"), ("{", "}"), ("⟨", "⟩")];

impl Parser {
    fn err<T>(&self, msg: impl Into<String>) -> Res<T> {
        Err(Error {
            message: msg.into(),
            offset: self.i,
        })
    }

    fn peek(&self) -> Option<char> {
        self.s.get(self.i).copied()
    }

    fn peek_at(&self, k: usize) -> Option<char> {
        self.s.get(self.i + k).copied()
    }

    fn skip_ws(&mut self) {
        while let Some(c) = self.peek() {
            if c.is_whitespace() {
                self.i += 1;
            } else if c == '%' {
                while let Some(c) = self.peek() {
                    self.i += 1;
                    if c == '\n' {
                        break;
                    }
                }
            } else {
                break;
            }
        }
    }

    /// After a `\`: the command name (letters, or one other character).
    fn cmd_name(&mut self) -> String {
        let mut name = String::new();
        while let Some(c) = self.peek() {
            if c.is_ascii_alphabetic() {
                name.push(c);
                self.i += 1;
            } else {
                break;
            }
        }
        if name.is_empty()
            && let Some(c) = self.peek()
        {
            name.push(c);
            self.i += 1;
        } else if self.peek() == Some('*')
            && matches!(name.as_str(), "operatorname" | "tag" | "hspace")
        {
            self.i += 1;
        }
        name
    }

    /// The name of the command at the cursor, without moving.
    fn peek_cmd(&self) -> Option<String> {
        if self.peek() != Some('\\') {
            return None;
        }
        let mut name = String::new();
        let mut k = self.i + 1;
        while let Some(&c) = self.s.get(k) {
            if c.is_ascii_alphabetic() {
                name.push(c);
                k += 1;
            } else {
                break;
            }
        }
        if name.is_empty() {
            name.extend(self.s.get(self.i + 1));
        }
        Some(name)
    }

    fn top(&mut self) -> Res<Row> {
        let mut rows: Vec<Row> = Vec::new();
        let mut cur = Row::new();
        loop {
            let (row, hit) = self.seq(Close::End, None)?;
            cur.extend(row);
            match hit {
                Hit::End => break,
                Hit::Amp => cur.push(Node::Text(Run::new("&"))),
                Hit::NewRow => rows.push(normalize(std::mem::take(&mut cur))),
                _ => return self.err("unexpected command"),
            }
        }
        let cur = normalize(cur);
        if rows.is_empty() {
            return Ok(cur);
        }
        if !cur.is_empty() {
            rows.push(cur);
        }
        Ok(vec![Node::EqArray { rows }])
    }

    /// Items up to a closing token. `stop`: a nary operand (`Some(true)`
    /// also stops before + and −) or function argument ends before
    /// relations; closing tokens then end it without being consumed.
    fn seq(&mut self, close: Close, stop: Option<bool>) -> Res<(Row, Hit)> {
        let mut items: Row = Vec::new();
        loop {
            self.skip_ws();
            let Some(c) = self.peek() else {
                if close == Close::End || stop.is_some() {
                    return Ok((ungroup(items), Hit::End));
                }
                return self.err(match close {
                    Close::Brace => "missing }",
                    Close::Bracket => "missing ]",
                    Close::Right => "missing \\right",
                    _ => "missing \\end",
                });
            };
            if let Some(bin) = stop {
                let ends = match c {
                    '}' | '&' => true,
                    ']' => close == Close::Bracket,
                    '\\' => matches!(
                        self.peek_cmd().as_deref(),
                        Some("\\" | "right" | "middle" | "end" | "cr" | "tag")
                    ),
                    c => {
                        let cl = symbols::class(c);
                        cl == Class::Rel
                            || (bin && cl == Class::Bin && matches!(c, '+' | '-' | '−' | '±' | '∓'))
                    }
                };
                let ends = ends
                    || self
                        .peek_cmd()
                        .and_then(|n| symbols::symbol(&n))
                        .is_some_and(|ch| {
                            let cl = symbols::class(ch);
                            cl == Class::Rel || (bin && matches!(ch, '±' | '∓'))
                        });
                if ends {
                    return Ok((ungroup(items), Hit::Stop));
                }
            }
            match c {
                '}' => {
                    if close == Close::Brace {
                        self.i += 1;
                        return Ok((ungroup(items), Hit::End));
                    }
                    return self.err("unexpected }");
                }
                ']' if close == Close::Bracket => {
                    self.i += 1;
                    return Ok((ungroup(items), Hit::End));
                }
                '&' => {
                    if matches!(close, Close::Env | Close::End) {
                        self.i += 1;
                        return Ok((ungroup(items), Hit::Amp));
                    }
                    self.i += 1;
                    items.push(Node::Text(Run::new("&")));
                }
                '^' | '_' => {
                    self.i += 1;
                    let arg = self.arg()?;
                    attach(&mut items, c == '^', arg);
                }
                '\'' => {
                    self.i += 1;
                    let mut n = 1;
                    while self.peek() == Some('\'') {
                        self.i += 1;
                        n += 1;
                    }
                    let p = match n {
                        1 => "′",
                        2 => "″",
                        3 => "‴",
                        _ => "⁗",
                    };
                    push_item(&mut items, Node::Text(Run::new(p)));
                }
                '\\' => {
                    let start = self.i;
                    self.i += 1;
                    let name = self.cmd_name();
                    match name.as_str() {
                        "\\" | "cr" => {
                            if matches!(close, Close::Env | Close::End) {
                                // An optional spacing argument: `\\[2pt]`.
                                if self.peek() == Some('[') {
                                    while let Some(c) = self.peek() {
                                        self.i += 1;
                                        if c == ']' {
                                            break;
                                        }
                                    }
                                }
                                return Ok((ungroup(items), Hit::NewRow));
                            }
                            // A line break elsewhere: ignored.
                        }
                        "right" => {
                            if close == Close::Right {
                                let d = self.delim()?;
                                return Ok((ungroup(items), Hit::Right(d)));
                            }
                            self.i = start;
                            return self.err("\\right without \\left");
                        }
                        "middle" => {
                            if close == Close::Right {
                                let d = self.delim()?;
                                return Ok((ungroup(items), Hit::Middle(d)));
                            }
                            let d = self.delim()?;
                            items.push(Node::Text(Run::new(d)));
                        }
                        "end" => {
                            let env = self.braced_word()?;
                            if close == Close::Env {
                                return Ok((ungroup(items), Hit::EndEnv(env)));
                            }
                            self.i = start;
                            return self.err(format!("\\end{{{env}}} without \\begin"));
                        }
                        _ => {
                            for n in self.command(&name, start)? {
                                push_item(&mut items, n);
                            }
                        }
                    }
                }
                '{' => {
                    self.i += 1;
                    let (row, _) = self.seq(Close::Brace, None)?;
                    // `{}_a^b X`: scripts before.
                    if row.is_empty()
                        && let Some(pre) = self.prescripts()?
                    {
                        items.push(pre);
                        continue;
                    }
                    items.push(Node::Group { body: row });
                }
                '$' => self.i += 1,
                '~' => {
                    self.i += 1;
                    items.push(Node::Text(Run::new("\u{00A0}")));
                }
                c => {
                    self.i += 1;
                    let c = match c {
                        '-' => '−',
                        '*' => '∗',
                        c => c,
                    };
                    push_item(&mut items, Node::Text(Run::new(c.to_string())));
                }
            }
        }
    }

    /// `{}_a^b X` after the empty group: the scripts and the base.
    fn prescripts(&mut self) -> Res<Option<Node>> {
        let save = self.i;
        let mut sub = None;
        let mut sup = None;
        for _ in 0..2 {
            self.skip_ws();
            match self.peek() {
                Some('_') if sub.is_none() => {
                    self.i += 1;
                    sub = Some(self.arg()?);
                }
                Some('^') if sup.is_none() => {
                    self.i += 1;
                    sup = Some(self.arg()?);
                }
                _ => break,
            }
        }
        if sub.is_none() && sup.is_none() {
            self.i = save;
            return Ok(None);
        }
        self.skip_ws();
        if self.peek().is_none() || matches!(self.peek(), Some('}' | '&')) {
            self.i = save;
            return Ok(None);
        }
        let base = self.term()?;
        Ok(Some(Node::PreScripts {
            sub: sub.unwrap_or_default(),
            sup: sup.unwrap_or_default(),
            base,
        }))
    }

    /// A command argument or script: a group, a command, or one character.
    fn arg(&mut self) -> Res<Row> {
        self.skip_ws();
        match self.peek() {
            None => self.err("missing argument"),
            Some('{') => {
                self.i += 1;
                Ok(ungroup(self.seq(Close::Brace, None)?.0))
            }
            Some('\\') => {
                let start = self.i;
                self.i += 1;
                let name = self.cmd_name();
                if matches!(name.as_str(), "\\" | "right" | "end" | "middle") {
                    self.i = start;
                    return self.err("missing argument");
                }
                Ok(normalize(self.command(&name, start)?))
            }
            Some('}' | '&' | '^' | '_') => self.err("missing argument"),
            Some(c) => {
                self.i += 1;
                let c = if c == '-' { '−' } else { c };
                Ok(vec![Node::Text(Run::new(c.to_string()))])
            }
        }
    }

    /// One item with its scripts (a function's argument).
    fn term(&mut self) -> Res<Row> {
        self.skip_ws();
        let mut items = Row::new();
        match self.peek() {
            Some('(' | '[') => {
                // Up to the matching bracket.
                let mut tmp = Row::new();
                let mut guard = 0;
                loop {
                    let (mut row, hit) = self.seq_one()?;
                    for n in row.drain(..) {
                        push_item(&mut tmp, n);
                    }
                    guard += 1;
                    let done = tmp.len() == 1 && matches!(tmp[0], Node::Delim { .. });
                    if done || hit || guard > 10_000 {
                        break;
                    }
                }
                items.extend(tmp);
            }
            _ => {
                let a = self.arg()?;
                items.push(Node::Group { body: a });
            }
        }
        // Scripts on it.
        loop {
            self.skip_ws();
            match self.peek() {
                Some(c @ ('^' | '_')) => {
                    self.i += 1;
                    let a = self.arg()?;
                    attach(&mut items, c == '^', a);
                }
                _ => break,
            }
        }
        Ok(ungroup(items))
    }

    /// One item (for [`Self::term`]): `true` when input or a closing
    /// token is reached.
    fn seq_one(&mut self) -> Res<(Row, bool)> {
        self.skip_ws();
        match self.peek() {
            None | Some('}' | '&') => Ok((Row::new(), true)),
            Some('\\')
                if matches!(
                    self.peek_cmd().as_deref(),
                    Some("\\" | "right" | "middle" | "end" | "cr")
                ) =>
            {
                Ok((Row::new(), true))
            }
            Some(c @ ('^' | '_')) => {
                self.i += 1;
                let a = self.arg()?;
                Ok((
                    vec![Node::Scripts {
                        base: Row::new(),
                        sub: (c == '_').then(|| a.clone()),
                        sup: (c == '^').then_some(a),
                    }],
                    false,
                ))
            }
            Some(_) => {
                let a = self.arg()?;
                Ok((a, false))
            }
        }
    }

    /// A delimiter after `\left`, `\right`, `\big`…: `""` for `.`.
    fn delim(&mut self) -> Res<String> {
        self.skip_ws();
        match self.peek() {
            None => self.err("missing delimiter"),
            Some('.') => {
                self.i += 1;
                Ok(String::new())
            }
            Some('\\') => {
                self.i += 1;
                let name = self.cmd_name();
                Ok(match name.as_str() {
                    "{" | "lbrace" => "{".into(),
                    "}" | "rbrace" => "}".into(),
                    "|" | "Vert" | "lVert" | "rVert" => "‖".into(),
                    "vert" | "lvert" | "rvert" => "|".into(),
                    "llbracket" => "⟦".into(),
                    "rrbracket" => "⟧".into(),
                    n => match symbols::symbol(n) {
                        Some(c) => c.to_string(),
                        None => return self.err(format!("unknown delimiter \\{n}")),
                    },
                })
            }
            Some(c) => {
                self.i += 1;
                Ok(c.to_string())
            }
        }
    }

    /// `{word}` (an environment name).
    fn braced_word(&mut self) -> Res<String> {
        self.skip_ws();
        if self.peek() != Some('{') {
            return self.err("missing {");
        }
        self.i += 1;
        let mut w = String::new();
        while let Some(c) = self.peek() {
            self.i += 1;
            if c == '}' {
                return Ok(w.trim().to_owned());
            }
            w.push(c);
        }
        self.err("missing }")
    }

    /// Raw text up to the matching brace (`\text{…}`).
    fn braced_text(&mut self) -> Res<String> {
        self.skip_ws();
        if self.peek() != Some('{') {
            // `\text x`: one character.
            return match self.peek() {
                Some(c) => {
                    self.i += 1;
                    Ok(c.to_string())
                }
                None => self.err("missing argument"),
            };
        }
        self.i += 1;
        let mut depth = 0;
        let mut w = String::new();
        while let Some(c) = self.peek() {
            self.i += 1;
            match c {
                '{' => {
                    depth += 1;
                }
                '}' if depth == 0 => return Ok(w),
                '}' => depth -= 1,
                '\\' => {
                    let name = self.cmd_name();
                    match name.as_str() {
                        "{" | "}" | "%" | "$" | "&" | "#" | "_" => w.push_str(&name),
                        " " | "," | ";" | ":" | "quad" => w.push(' '),
                        n => {
                            if let Some(ch) = symbols::symbol(n) {
                                w.push(ch);
                            }
                        }
                    }
                    continue;
                }
                '$' => continue,
                '~' => {
                    w.push('\u{00A0}');
                    continue;
                }
                _ => {}
            }
            if !(matches!(c, '{' | '}')) {
                w.push(c);
            }
        }
        self.err("missing }")
    }

    /// An optional `[…]` argument.
    fn optional(&mut self) -> Res<Option<Row>> {
        self.skip_ws();
        if self.peek() != Some('[') {
            return Ok(None);
        }
        self.i += 1;
        Ok(Some(ungroup(self.seq(Close::Bracket, None)?.0)))
    }

    /// Big operator limits and operand.
    fn nary(&mut self, op: char) -> Res<Vec<Node>> {
        let mut under_over = None;
        let mut sub = None;
        let mut sup = None;
        loop {
            self.skip_ws();
            match self.peek() {
                Some('_') if sub.is_none() => {
                    self.i += 1;
                    sub = Some(self.arg()?);
                }
                Some('^') if sup.is_none() => {
                    self.i += 1;
                    sup = Some(self.arg()?);
                }
                Some('\\') => match self.peek_cmd().as_deref() {
                    Some("limits") => {
                        self.i += 7;
                        under_over = Some(true);
                    }
                    Some("nolimits") => {
                        self.i += 9;
                        under_over = Some(false);
                    }
                    _ => break,
                },
                _ => break,
            }
        }
        let body = self.operand(!symbols::is_integral(op))?;
        Ok(vec![Node::Nary {
            op,
            under_over: under_over.unwrap_or(!symbols::is_integral(op)),
            sub,
            sup,
            body,
        }])
    }

    /// A big operator's operand: a group, else items up to a relation (or
    /// a + or −, for sums).
    fn operand(&mut self, stop_bin: bool) -> Res<Row> {
        self.skip_ws();
        if self.peek() == Some('{') {
            let save = self.i;
            self.i += 1;
            let (row, _) = self.seq(Close::Brace, None)?;
            self.skip_ws();
            // `{a}^2` is not the whole operand.
            if !matches!(self.peek(), Some('^' | '_')) {
                return Ok(ungroup(row));
            }
            self.i = save;
        }
        let (row, _) = self.seq(Close::End, Some(stop_bin))?;
        Ok(row)
    }

    /// A function name's scripts or limits, then its argument.
    fn func(&mut self, name: &str) -> Res<Vec<Node>> {
        let mut name_row = vec![Node::Text(Run::styled(name, Style::PLAIN))];
        let limits = symbols::LIMIT_FUNCTIONS.contains(&name);
        let mut sub = None;
        let mut sup = None;
        let mut explicit = None;
        loop {
            self.skip_ws();
            match self.peek() {
                Some('_') if sub.is_none() => {
                    self.i += 1;
                    sub = Some(self.arg()?);
                }
                Some('^') if sup.is_none() => {
                    self.i += 1;
                    sup = Some(self.arg()?);
                }
                Some('\\') => match self.peek_cmd().as_deref() {
                    Some("limits") => {
                        self.i += 7;
                        explicit = Some(true);
                    }
                    Some("nolimits") => {
                        self.i += 9;
                        explicit = Some(false);
                    }
                    _ => break,
                },
                _ => break,
            }
        }
        if explicit.unwrap_or(limits) && sup.is_none() {
            if let Some(s) = sub.take() {
                name_row = vec![Node::Limit {
                    upper: false,
                    base: name_row,
                    limit: s,
                }];
            }
        } else if sub.is_some() || sup.is_some() {
            name_row = vec![Node::Scripts {
                base: name_row,
                sub,
                sup,
            }];
        }
        self.skip_ws();
        let ends = match self.peek() {
            None | Some('}' | '&' | ']') => true,
            Some('\\') => matches!(
                self.peek_cmd().as_deref(),
                Some("\\" | "right" | "middle" | "end" | "cr")
            ),
            Some(c) => matches!(symbols::class(c), Class::Rel | Class::Punct | Class::Close),
        };
        let body = if ends {
            Row::new()
        } else if limits {
            // `\lim_{x→0} f(x) + 1`: up to a relation or + / −.
            self.operand(true)?
        } else {
            self.term()?
        };
        Ok(vec![Node::Func {
            name: name_row,
            body,
        }])
    }

    /// An environment after `\begin{name}`.
    fn env(&mut self, name: &str) -> Res<Vec<Node>> {
        let base = name.trim_end_matches('*');
        if matches!(base, "array" | "alignat" | "alignedat" | "subarray") {
            // Column spec / count.
            self.skip_ws();
            if self.peek() == Some('{') {
                self.braced_text()?;
            }
        }
        let mut rows: Vec<Vec<Row>> = Vec::new();
        let mut cells: Vec<Row> = Vec::new();
        loop {
            let (cell, hit) = self.seq(Close::Env, None)?;
            match hit {
                Hit::Amp => cells.push(cell),
                Hit::NewRow => {
                    cells.push(cell);
                    rows.push(std::mem::take(&mut cells));
                }
                Hit::EndEnv(e) => {
                    if e != name {
                        return self.err(format!("\\begin{{{name}}} ended by \\end{{{e}}}"));
                    }
                    if !(cells.is_empty() && cell.is_empty()) {
                        cells.push(cell);
                    }
                    if !cells.is_empty() {
                        rows.push(cells);
                    }
                    break;
                }
                _ => return self.err("missing \\end"),
            }
        }
        // A trailing `\\` leaves an empty last row.
        while rows.last().is_some_and(|r| r.iter().all(|c| c.is_empty())) {
            rows.pop();
        }
        let matrix = |rows: Vec<Vec<Row>>| Node::Matrix { rows };
        let joined = |rows: Vec<Vec<Row>>| {
            rows.into_iter()
                .map(|cells| {
                    let mut r = Row::new();
                    for (k, c) in cells.into_iter().enumerate() {
                        if k > 0 {
                            r.push(Node::Text(Run::new("&")));
                        }
                        r.extend(c);
                    }
                    normalize(r)
                })
                .collect::<Vec<Row>>()
        };
        let fence = |open: &str, close: &str, n: Node| Node::Delim {
            open: open.into(),
            close: close.into(),
            sep: "|".into(),
            items: vec![vec![n]],
            grow: true,
        };
        Ok(vec![match base {
            "matrix" | "smallmatrix" | "array" | "subarray" => matrix(rows),
            "pmatrix" => fence("(", ")", matrix(rows)),
            "bmatrix" => fence("[", "]", matrix(rows)),
            "Bmatrix" => fence("{", "}", matrix(rows)),
            "vmatrix" => fence("|", "|", matrix(rows)),
            "Vmatrix" => fence("‖", "‖", matrix(rows)),
            "cases" | "dcases" => fence("{", "", Node::EqArray { rows: joined(rows) }),
            "rcases" => fence("", "}", Node::EqArray { rows: joined(rows) }),
            "aligned" | "align" | "split" | "gathered" | "gather" | "eqnarray" | "alignat"
            | "alignedat" | "multline" | "flalign" => Node::EqArray { rows: joined(rows) },
            "equation" | "displaymath" | "math" => {
                let mut r = Row::new();
                for (k, row) in joined(rows).into_iter().enumerate() {
                    if k > 0 {
                        r.push(Node::Text(Run::new(" ")));
                    }
                    r.extend(row);
                }
                Node::Group { body: r }
            }
            _ => return self.err(format!("unknown environment {name}")),
        }])
    }

    /// The nodes a command stands for (scripts after it are attached by
    /// the caller).
    fn command(&mut self, name: &str, start: usize) -> Res<Vec<Node>> {
        let text = |s: &str| vec![Node::Text(Run::new(s))];
        if let Some(sp) = symbols::space(name) {
            return Ok(vec![Node::Text(Run::new(sp.to_string()))]);
        }
        if let Some(c) = symbols::symbol(name) {
            if symbols::is_nary(c) {
                return self.nary(c);
            }
            return Ok(text(&c.to_string()));
        }
        if symbols::FUNCTIONS.contains(&name) {
            return self.func(name);
        }
        if let Some(acc) = symbols::accent(name) {
            let body = self.arg()?;
            return Ok(vec![Node::Accent { chr: acc, body }]);
        }
        let styled = |p: &mut Parser, style: Style| -> Res<Vec<Node>> {
            let row = p.arg()?;
            Ok(restyle(row, style))
        };
        Ok(match name {
            "{" | "}" | "%" | "$" | "#" | "_" | "&" | "|" => {
                let c = if name == "|" { "‖" } else { name };
                text(c)
            }
            "frac" | "dfrac" | "tfrac" | "cfrac" => {
                let num = self.arg()?;
                let den = self.arg()?;
                vec![Node::Frac {
                    kind: FracKind::Bar,
                    num,
                    den,
                }]
            }
            "sfrac" | "nicefrac" | "xfrac" => {
                let num = self.arg()?;
                let den = self.arg()?;
                vec![Node::Frac {
                    kind: FracKind::Skewed,
                    num,
                    den,
                }]
            }
            "binom" | "dbinom" | "tbinom" => {
                let num = self.arg()?;
                let den = self.arg()?;
                vec![Node::Delim {
                    open: "(".into(),
                    close: ")".into(),
                    sep: "|".into(),
                    items: vec![vec![Node::Frac {
                        kind: FracKind::NoBar,
                        num,
                        den,
                    }]],
                    grow: true,
                }]
            }
            "genfrac" => {
                let open = self.braced_text()?;
                let close = self.braced_text()?;
                let thick = self.braced_text()?;
                self.braced_text()?;
                let num = self.arg()?;
                let den = self.arg()?;
                let kind = if thick.trim().trim_end_matches("pt").trim() == "0" {
                    FracKind::NoBar
                } else {
                    FracKind::Bar
                };
                let f = Node::Frac { kind, num, den };
                if open.is_empty() && close.is_empty() {
                    vec![f]
                } else {
                    let d = |s: &str| match s.trim() {
                        "." => String::new(),
                        "\\{" | "\\lbrace" => "{".into(),
                        "\\}" | "\\rbrace" => "}".into(),
                        s => s.into(),
                    };
                    vec![Node::Delim {
                        open: d(&open),
                        close: d(&close),
                        sep: "|".into(),
                        items: vec![vec![f]],
                        grow: true,
                    }]
                }
            }
            "atop" | "over" | "choose" => {
                return self.err(format!("\\{name} is not supported; use \\frac or \\binom"));
            }
            "sqrt" => {
                let degree = self.optional()?.filter(|d| !d.is_empty());
                let body = self.arg()?;
                vec![Node::Radical { degree, body }]
            }
            "root" => {
                // `\root 3 \of {x}`.
                let mut deg = Row::new();
                loop {
                    self.skip_ws();
                    if self.peek_cmd().as_deref() == Some("of") {
                        self.i += 3;
                        break;
                    }
                    if self.peek().is_none() {
                        return self.err("\\root without \\of");
                    }
                    deg.extend(self.arg()?);
                }
                let body = self.arg()?;
                vec![Node::Radical {
                    degree: Some(deg),
                    body,
                }]
            }
            "overline" => vec![Node::Bar {
                top: true,
                body: self.arg()?,
            }],
            "underline" => vec![Node::Bar {
                top: false,
                body: self.arg()?,
            }],
            "overbrace" | "underbrace" | "overparen" | "underparen" | "overbracket"
            | "underbracket" => {
                let top = name.starts_with("over");
                let chr = match name {
                    "overbrace" => '⏞',
                    "underbrace" => '⏟',
                    "overparen" => '⏜',
                    "underparen" => '⏝',
                    "overbracket" => '⎴',
                    _ => '⎵',
                };
                let body = self.arg()?;
                let g = Node::GroupChr { chr, top, body };
                self.skip_ws();
                let label = match self.peek() {
                    Some('^') if top => {
                        self.i += 1;
                        Some(self.arg()?)
                    }
                    Some('_') if !top => {
                        self.i += 1;
                        Some(self.arg()?)
                    }
                    _ => None,
                };
                match label {
                    Some(limit) => vec![Node::Limit {
                        upper: top,
                        base: vec![g],
                        limit,
                    }],
                    None => vec![g],
                }
            }
            "overset" | "stackrel" | "underset" => {
                let limit = self.arg()?;
                let base = self.arg()?;
                vec![Node::Limit {
                    upper: name != "underset",
                    base,
                    limit,
                }]
            }
            "prescript" => {
                let sup = self.arg()?;
                let sub = self.arg()?;
                let base = self.arg()?;
                vec![Node::PreScripts { sub, sup, base }]
            }
            "boxed" | "fbox" => vec![Node::Boxed { body: self.arg()? }],
            "phantom" | "hphantom" | "vphantom" => vec![Node::Phantom {
                body: self.arg()?,
                show: false,
                zero_width: name == "vphantom",
                zero_asc: name == "hphantom",
                zero_desc: name == "hphantom",
            }],
            "text" | "textrm" | "mbox" | "textnormal" | "textup" | "hbox" => {
                let t = self.braced_text()?;
                vec![Node::Text(Run::styled(t, Style::TEXT))]
            }
            "textbf" => {
                let t = self.braced_text()?;
                vec![Node::Text(Run::styled(
                    t,
                    Style {
                        weight: Some(Weight::Bold),
                        ..Style::TEXT
                    },
                ))]
            }
            "textit" | "emph" => {
                let t = self.braced_text()?;
                vec![Node::Text(Run::styled(
                    t,
                    Style {
                        weight: Some(Weight::Italic),
                        ..Style::TEXT
                    },
                ))]
            }
            "operatorname" | "mathop" => {
                let t = self.braced_text()?;
                if name == "mathop" {
                    vec![Node::Text(Run::styled(t, Style::PLAIN))]
                } else {
                    self.func_named(&t)?
                }
            }
            "mathrm" | "rm" | "mathup" => styled(self, Style::PLAIN)?,
            "mathbf" | "bf" => styled(
                self,
                Style {
                    weight: Some(Weight::Bold),
                    ..Style::default()
                },
            )?,
            "boldsymbol" | "bm" | "mathbfit" => styled(
                self,
                Style {
                    weight: Some(Weight::BoldItalic),
                    ..Style::default()
                },
            )?,
            "mathit" | "it" => styled(
                self,
                Style {
                    weight: Some(Weight::Italic),
                    ..Style::default()
                },
            )?,
            "mathnormal" => styled(self, Style::default())?,
            "mathcal" | "mathscr" => styled(
                self,
                Style {
                    variant: Variant::Script,
                    weight: Some(Weight::Plain),
                    normal: false,
                },
            )?,
            "mathfrak" => styled(
                self,
                Style {
                    variant: Variant::Fraktur,
                    weight: Some(Weight::Plain),
                    normal: false,
                },
            )?,
            "mathbb" | "Bbb" => styled(
                self,
                Style {
                    variant: Variant::DoubleStruck,
                    weight: Some(Weight::Plain),
                    normal: false,
                },
            )?,
            "mathsf" => styled(
                self,
                Style {
                    variant: Variant::SansSerif,
                    weight: Some(Weight::Plain),
                    normal: false,
                },
            )?,
            "mathtt" => styled(
                self,
                Style {
                    variant: Variant::Monospace,
                    weight: Some(Weight::Plain),
                    normal: false,
                },
            )?,
            "left" => {
                let open = self.delim()?;
                let mut items = Vec::new();
                let mut sep = String::from("|");
                let close;
                loop {
                    let (row, hit) = self.seq(Close::Right, None)?;
                    items.push(row);
                    match hit {
                        Hit::Middle(d) => sep = d,
                        Hit::Right(d) => {
                            close = d;
                            break;
                        }
                        _ => return self.err("missing \\right"),
                    }
                }
                vec![Node::Delim {
                    open,
                    close,
                    sep,
                    items,
                    grow: true,
                }]
            }
            "big" | "Big" | "bigg" | "Bigg" | "bigl" | "bigr" | "bigm" | "Bigl" | "Bigr"
            | "Bigm" | "biggl" | "biggr" | "biggm" | "Biggl" | "Biggr" | "Biggm" => {
                let d = self.delim()?;
                text(&d)
            }
            "begin" => {
                let env = self.braced_word()?;
                self.env(&env)?
            }
            "tag" | "tag*" => {
                let t = self.braced_text()?;
                text(&format!("#({t})"))
            }
            "not" => {
                self.skip_ws();
                let next = self.arg()?;
                let c = match next.as_slice() {
                    [Node::Text(r)] => r.text.clone(),
                    _ => String::new(),
                };
                let neg = match c.as_str() {
                    "=" => "≠".to_owned(),
                    "<" => "≮".to_owned(),
                    ">" => "≯".to_owned(),
                    "≤" => "≰".to_owned(),
                    "≥" => "≱".to_owned(),
                    "∈" => "∉".to_owned(),
                    "∼" => "≁".to_owned(),
                    "≅" => "≇".to_owned(),
                    "⊆" => "⊈".to_owned(),
                    "⊇" => "⊉".to_owned(),
                    "∣" => "∤".to_owned(),
                    "∥" => "∦".to_owned(),
                    "≡" => "≢".to_owned(),
                    "" => "/".to_owned(),
                    c => format!("{c}\u{0338}"),
                };
                text(&neg)
            }
            "limits" | "nolimits" | "displaystyle" | "textstyle" | "scriptstyle"
            | "scriptscriptstyle" | "nonumber" | "notag" | "left." | "label" | "relax"
            | "allowbreak" | "nobreak" => {
                if name == "label" {
                    self.braced_text()?;
                }
                Vec::new()
            }
            "hspace" | "hspace*" | "kern" | "mkern" | "mspace" => {
                if self.peek() == Some('{') {
                    self.braced_text()?;
                } else {
                    // `\kern3mu`: up to the unit.
                    while let Some(c) = self.peek() {
                        if c.is_ascii_alphabetic()
                            && self.peek_at(1).is_some_and(|d| d.is_ascii_alphabetic())
                        {
                            self.i += 2;
                            break;
                        }
                        self.i += 1;
                    }
                }
                text("\u{2003}")
            }
            "color" => {
                self.braced_text()?;
                Vec::new()
            }
            "textcolor" | "colorbox" => {
                self.braced_text()?;
                self.arg()?
            }
            "mathbin" | "mathrel" | "mathord" | "mathopen" | "mathclose" | "mathpunct"
            | "mathinner" | "ensuremath" => self.arg()?,
            "dots" | "ldots" => text("…"),
            "iff" => text("⟺"),
            "lbrace" => text("{"),
            "rbrace" => text("}"),
            _ => {
                self.i = start;
                return self.err(format!("unknown command \\{name}"));
            }
        })
    }

    /// `\operatorname{name}`: a function with that name.
    fn func_named(&mut self, name: &str) -> Res<Vec<Node>> {
        self.func(name)
    }
}

/// A group's items when it's only a wrapper.
fn ungroup(row: Row) -> Row {
    let mut out = Row::new();
    for n in row {
        match n {
            Node::Group { body } => {
                for b in body {
                    push(&mut out, b);
                }
            }
            n => push(&mut out, n),
        }
    }
    out
}

/// Pushes an item, closing a bracket pair (`(…)` becomes a delimiter).
fn push_item(items: &mut Row, n: Node) {
    if let Node::Text(r) = &n
        && r.style.is_default()
        && let Some(&(open, _)) = OPENS.iter().find(|(_, c)| *c == r.text)
    {
        let found = items
            .iter()
            .rposition(|x| matches!(x, Node::Text(o) if o.style.is_default() && o.text == open));
        if let Some(j) = found {
            let inner: Row = ungroup(items.drain(j + 1..).collect());
            items.pop();
            let close = r.text.clone();
            items.push(Node::Delim {
                open: open.to_owned(),
                close,
                sep: "|".into(),
                items: vec![inner],
                grow: true,
            });
            return;
        }
    }
    items.push(n);
}

/// Attaches a script to the last item.
fn attach(items: &mut Row, sup: bool, arg: Row) {
    match items.last_mut() {
        Some(Node::Scripts { sub: s @ None, .. }) if !sup => {
            *s = Some(arg);
            return;
        }
        Some(Node::Scripts { sup: s @ None, .. }) if sup => {
            *s = Some(arg);
            return;
        }
        Some(Node::Nary { sub: s @ None, .. }) if !sup => {
            *s = Some(arg);
            return;
        }
        Some(Node::Nary { sup: s @ None, .. }) if sup => {
            *s = Some(arg);
            return;
        }
        _ => {}
    }
    let base = match items.pop() {
        Some(Node::Group { body }) => body,
        // A run's last character only.
        Some(Node::Text(mut r)) if r.text.chars().count() > 1 => {
            let last = r.text.pop().map(|c| c.to_string()).unwrap_or_default();
            let style = r.style;
            items.push(Node::Text(r));
            vec![Node::Text(Run::styled(last, style))]
        }
        Some(n) => vec![n],
        None => Row::new(),
    };
    items.push(Node::Scripts {
        base,
        sub: (!sup).then(|| arg.clone()),
        sup: sup.then_some(arg),
    });
}

/// Applies a style to every run in a row.
fn restyle(row: Row, style: Style) -> Row {
    row.into_iter()
        .map(|n| match n {
            Node::Text(mut r) => {
                if !r.style.normal {
                    r.style = style;
                }
                Node::Text(r)
            }
            Node::Group { body } => Node::Group {
                body: restyle(body, style),
            },
            Node::Scripts { base, sub, sup } => Node::Scripts {
                base: restyle(base, style),
                sub,
                sup,
            },
            n => n,
        })
        .collect()
}

// ── writing ────────────────────────────────────────────────────────────

/// LaTeX for an equation (no surrounding `$`). Display lines are joined
/// with `\\`.
pub fn write(eq: &Equation) -> String {
    let lines: Vec<String> = eq.lines.iter().map(|l| write_row(l)).collect();
    if lines.len() > 1 {
        format!(
            "\\begin{{gathered}}{}\\end{{gathered}}",
            lines.join(" \\\\ ")
        )
    } else {
        lines.into_iter().next().unwrap_or_default()
    }
}

/// LaTeX for a row.
pub fn write_row(r: &[Node]) -> String {
    let mut w = W::default();
    w.row(r);
    w.out.trim().to_owned()
}

#[derive(Default)]
struct W {
    out: String,
}

impl W {
    /// Appends text, putting a space between a command word and a letter
    /// that follows it.
    fn put(&mut self, s: &str) {
        if s.is_empty() {
            return;
        }
        let first = s.chars().next().unwrap_or(' ');
        if first.is_ascii_alphabetic() && ends_in_command(&self.out) {
            self.out.push(' ');
        }
        self.out.push_str(s);
    }

    fn cmd(&mut self, name: &str) {
        self.out.push('\\');
        self.out.push_str(name);
    }

    fn group(&mut self, r: &[Node]) {
        self.out.push('{');
        self.row(r);
        self.out.push('}');
    }

    /// A script or argument: braces unless it's one character.
    fn arg(&mut self, r: &[Node]) {
        if let [Node::Text(t)] = r
            && t.style.is_default()
            && t.text.chars().count() == 1
        {
            let c = t.text.chars().next().unwrap_or(' ');
            if c.is_ascii_alphanumeric() {
                self.out.push(c);
                return;
            }
        }
        self.group(r);
    }

    fn row(&mut self, r: &[Node]) {
        for (k, n) in r.iter().enumerate() {
            // `#` starts the equation number: the rest of the row.
            if let Node::Text(t) = n
                && !t.style.normal
                && let Some(at) = t.text.find('#')
            {
                if at > 0 {
                    self.text(&Run::styled(&t.text[..at], t.style));
                }
                let mut num = t.text[at + 1..].to_owned();
                num.push_str(&plain_text(&r[k + 1..]));
                let num = num.trim();
                let num = num
                    .strip_prefix('(')
                    .and_then(|x| x.strip_suffix(')'))
                    .unwrap_or(num);
                self.out.push(' ');
                self.cmd("tag");
                self.out.push('{');
                self.out.push_str(num);
                self.out.push('}');
                return;
            }
            self.node(n);
        }
    }

    fn text(&mut self, run: &Run) {
        let s = &run.style;
        if s.normal {
            let cmd = match s.weight {
                Some(Weight::Bold) => "textbf",
                Some(Weight::Italic) => "textit",
                _ => "text",
            };
            self.cmd(cmd);
            self.out.push('{');
            for c in run.text.chars() {
                match c {
                    '{' | '}' | '%' | '$' | '&' | '#' | '_' => {
                        self.out.push('\\');
                        self.out.push(c);
                    }
                    '\\' => self.out.push_str("\\backslash "),
                    c => self.out.push(c),
                }
            }
            self.out.push('}');
            return;
        }
        let wrapper = match (s.variant, s.weight) {
            (Variant::Roman, None | Some(Weight::Italic)) => None,
            (Variant::Roman, Some(Weight::Plain)) => {
                if run.text.chars().any(|c| c.is_alphabetic()) {
                    Some("mathrm")
                } else {
                    None
                }
            }
            (Variant::Roman, Some(Weight::Bold)) => Some("mathbf"),
            (Variant::Roman, Some(Weight::BoldItalic)) => Some("boldsymbol"),
            (Variant::Script, _) => Some("mathcal"),
            (Variant::Fraktur, _) => Some("mathfrak"),
            (Variant::DoubleStruck, _) => Some("mathbb"),
            (Variant::SansSerif, _) => Some("mathsf"),
            (Variant::Monospace, _) => Some("mathtt"),
        };
        if let Some(wr) = wrapper {
            self.cmd(wr);
            self.out.push('{');
            self.chars(&run.text);
            self.out.push('}');
        } else {
            self.chars(&run.text);
        }
    }

    fn chars(&mut self, text: &str) {
        for c in text.chars() {
            let (base, v, bold, _) = alnum::unstyle(c);
            if base != c && v != Variant::Roman || bold {
                let wr = match v {
                    Variant::DoubleStruck => "mathbb",
                    Variant::Script => "mathcal",
                    Variant::Fraktur => "mathfrak",
                    Variant::SansSerif => "mathsf",
                    Variant::Monospace => "mathtt",
                    Variant::Roman => "mathbf",
                };
                self.cmd(wr);
                self.out.push('{');
                self.out.push(base);
                self.out.push('}');
                continue;
            }
            let c = base;
            match c {
                '{' | '}' | '%' | '$' | '_' => {
                    self.out.push('\\');
                    self.out.push(c);
                }
                '&' => self.out.push_str(" & "),
                '#' => self.out.push_str("\\#"),
                '^' => self.out.push_str("\\hat{}"),
                '\\' => self.cmd("backslash"),
                '−' => self.out.push('-'),
                '∗' => self.out.push('*'),
                '′' => self.out.push('\''),
                '″' => self.out.push_str("''"),
                '‴' => self.out.push_str("'''"),
                ' ' | '\t' | '\n' => {}
                '\u{00A0}' => self.out.push('~'),
                c => {
                    if let Some(sp) = symbols::space_command(c) {
                        self.cmd(sp);
                        if sp.chars().all(|x| x.is_ascii_alphabetic()) {
                            self.out.push(' ');
                        }
                    } else if let Some(name) = symbols::command_for(c) {
                        if matches!(c, '{' | '}' | '[' | ']' | '|' | ':') {
                            self.out.push(c);
                        } else {
                            self.cmd(name);
                        }
                    } else {
                        let mut b = [0u8; 4];
                        self.put(c.encode_utf8(&mut b));
                    }
                }
            }
        }
    }

    fn node(&mut self, n: &Node) {
        match n {
            Node::Text(r) => self.text(r),
            Node::Frac { kind, num, den } => match kind {
                FracKind::Bar => {
                    self.cmd("frac");
                    self.group(num);
                    self.group(den);
                }
                FracKind::Skewed => {
                    self.cmd("sfrac");
                    self.group(num);
                    self.group(den);
                }
                FracKind::Linear => {
                    self.group(num);
                    self.out.push('/');
                    self.group(den);
                }
                FracKind::NoBar => {
                    self.cmd("genfrac");
                    self.out.push_str("{}{}{0pt}{}");
                    self.group(num);
                    self.group(den);
                }
            },
            Node::Scripts { base, sub, sup } => {
                self.base(base);
                if let Some(s) = sub {
                    self.out.push('_');
                    self.arg(s);
                }
                if let Some(s) = sup {
                    self.out.push('^');
                    self.arg(s);
                }
            }
            Node::PreScripts { sub, sup, base } => {
                self.cmd("prescript");
                self.group(sup);
                self.group(sub);
                self.group(base);
            }
            Node::Radical { degree, body } => {
                self.cmd("sqrt");
                if let Some(d) = degree {
                    self.out.push('[');
                    self.row(d);
                    self.out.push(']');
                }
                self.group(body);
            }
            Node::Nary {
                op,
                under_over,
                sub,
                sup,
                body,
            } => {
                match symbols::command_for(*op) {
                    Some(name) => self.cmd(name),
                    None => self.put(&op.to_string()),
                }
                if *under_over == symbols::is_integral(*op) {
                    self.cmd(if *under_over { "limits" } else { "nolimits" });
                }
                if let Some(s) = sub {
                    self.out.push('_');
                    self.arg(s);
                }
                if let Some(s) = sup {
                    self.out.push('^');
                    self.arg(s);
                }
                self.out.push(' ');
                if needs_operand_braces(body, !symbols::is_integral(*op)) {
                    self.group(body);
                } else {
                    self.row(body);
                }
            }
            Node::Delim {
                open,
                close,
                sep,
                items,
                grow,
            } => {
                // Matrices and cases.
                if let [item] = items.as_slice()
                    && let [inner] = item.as_slice()
                {
                    let env = match (open.as_str(), close.as_str(), inner) {
                        ("(", ")", Node::Matrix { .. }) => Some("pmatrix"),
                        ("[", "]", Node::Matrix { .. }) => Some("bmatrix"),
                        ("{", "}", Node::Matrix { .. }) => Some("Bmatrix"),
                        ("|", "|", Node::Matrix { .. }) => Some("vmatrix"),
                        ("‖", "‖", Node::Matrix { .. }) => Some("Vmatrix"),
                        ("{", "", Node::EqArray { .. } | Node::Matrix { .. }) => Some("cases"),
                        ("", "}", Node::EqArray { .. } | Node::Matrix { .. }) => Some("rcases"),
                        _ => None,
                    };
                    if let Some(env) = env {
                        self.env(env, inner);
                        return;
                    }
                    if let (
                        "(",
                        ")",
                        Node::Frac {
                            kind: FracKind::NoBar,
                            num,
                            den,
                        },
                    ) = (open.as_str(), close.as_str(), inner)
                    {
                        self.cmd("binom");
                        self.group(num);
                        self.group(den);
                        return;
                    }
                }
                let simple = !grow
                    || (matches!((open.as_str(), close.as_str()), ("(", ")") | ("[", "]"))
                        && items.len() == 1
                        && !items.iter().any(|i| tall(i)));
                if simple && items.len() == 1 {
                    self.out.push_str(&plain_delim(open));
                    self.row(&items[0]);
                    self.out.push_str(&plain_delim(close));
                } else {
                    self.cmd("left");
                    self.out.push_str(&symbols::delim_latex(open));
                    for (k, i) in items.iter().enumerate() {
                        if k > 0 {
                            self.cmd("middle");
                            self.out.push_str(&symbols::delim_latex(sep));
                        }
                        self.out.push(' ');
                        self.row(i);
                    }
                    self.out.push(' ');
                    self.cmd("right");
                    self.out.push_str(&symbols::delim_latex(close));
                }
            }
            Node::Matrix { .. } => self.env("matrix", n),
            Node::EqArray { .. } => self.env("aligned", n),
            Node::Accent { chr, body } => {
                let name = symbols::accent_command(*chr).unwrap_or("hat");
                let wide = body.len() > 1
                    || matches!(body.as_slice(), [Node::Text(t)] if t.text.chars().count() > 1);
                let name = match (name, wide) {
                    ("hat", true) => "widehat",
                    ("tilde", true) => "widetilde",
                    (n, _) => n,
                };
                self.cmd(name);
                self.group(body);
            }
            Node::Bar { top, body } => {
                self.cmd(if *top { "overline" } else { "underline" });
                self.group(body);
            }
            Node::Boxed { body } => {
                self.cmd("boxed");
                self.group(body);
            }
            Node::Group { body } => self.group(body),
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
                    self.cmd(if *top { "overset" } else { "underset" });
                    self.out.push('{');
                    self.chars(&chr.to_string());
                    self.out.push('}');
                } else {
                    self.cmd(name);
                }
                self.group(body);
            }
            Node::Limit { upper, base, limit } => {
                // `\lim_{x\to 0}`, `\overbrace{…}^{…}`.
                if !upper && let Some(f) = function_name(base) {
                    self.cmd(&f);
                    self.out.push('_');
                    self.arg(limit);
                    return;
                }
                if let [Node::GroupChr { top, .. }] = base.as_slice()
                    && *top == *upper
                {
                    self.row(base);
                    self.out.push(if *upper { '^' } else { '_' });
                    self.arg(limit);
                    return;
                }
                self.cmd(if *upper { "overset" } else { "underset" });
                self.group(limit);
                self.group(base);
            }
            Node::Func { name, body } => {
                self.func_name(name);
                if body.is_empty() {
                    return;
                }
                let simple = body.len() == 1
                    && match &body[0] {
                        Node::Delim { .. } => true,
                        Node::Text(t) => t.text.chars().count() == 1,
                        _ => false,
                    };
                if simple {
                    if matches!(body[0], Node::Text(_)) {
                        self.out.push(' ');
                    }
                    self.row(body);
                } else {
                    self.group(body);
                }
            }
            Node::Phantom {
                body,
                zero_width,
                zero_asc,
                ..
            } => {
                self.cmd(if *zero_width {
                    "vphantom"
                } else if *zero_asc {
                    "hphantom"
                } else {
                    "phantom"
                });
                self.group(body);
            }
        }
    }

    /// A script base: braces around anything but one simple item.
    fn base(&mut self, base: &[Node]) {
        let one = match base {
            [Node::Text(t)] => t.text.chars().count() == 1 && !t.style.normal,
            [
                Node::Delim { .. } | Node::Frac { .. } | Node::Radical { .. } | Node::Accent { .. },
            ] => true,
            [] => {
                self.out.push_str("{}");
                return;
            }
            _ => false,
        };
        if one {
            self.row(base);
        } else {
            self.group(base);
        }
    }

    fn func_name(&mut self, name: &[Node]) {
        if let Some(f) = function_name(name) {
            self.cmd(&f);
            return;
        }
        match name {
            [Node::Scripts { base, sub, sup }] if function_name(base).is_some() => {
                self.func_name(base);
                if let Some(s) = sub {
                    self.out.push('_');
                    self.arg(s);
                }
                if let Some(s) = sup {
                    self.out.push('^');
                    self.arg(s);
                }
            }
            [Node::Limit { .. }] => self.row(name),
            _ => {
                let t = plain_text(name);
                if !t.is_empty() && t.chars().all(|c| c.is_alphabetic()) {
                    self.cmd("operatorname");
                    self.out.push('{');
                    self.out.push_str(&t);
                    self.out.push('}');
                } else {
                    self.row(name);
                }
            }
        }
    }

    fn env(&mut self, env: &str, n: &Node) {
        self.out.push_str(&format!("\\begin{{{env}}}"));
        match n {
            Node::Matrix { rows } => {
                for (k, r) in rows.iter().enumerate() {
                    if k > 0 {
                        self.out.push_str(" \\\\ ");
                    }
                    for (j, c) in r.iter().enumerate() {
                        if j > 0 {
                            self.out.push_str(" & ");
                        }
                        self.row(c);
                    }
                }
            }
            Node::EqArray { rows } => {
                for (k, r) in rows.iter().enumerate() {
                    if k > 0 {
                        self.out.push_str(" \\\\ ");
                    }
                    self.row(r);
                }
            }
            _ => {}
        }
        self.out.push_str(&format!("\\end{{{env}}}"));
    }
}

fn ends_in_command(s: &str) -> bool {
    let tail: String = s
        .chars()
        .rev()
        .take_while(|c| c.is_ascii_alphabetic())
        .collect();
    !tail.is_empty() && s[..s.len() - tail.len()].ends_with('\\')
}

fn plain_delim(s: &str) -> String {
    match s {
        "{" => "\\{".into(),
        "}" => "\\}".into(),
        "" => String::new(),
        s => symbols::delim_latex(s),
    }
}

/// A known function name (`sin`, `lim`) in plain text: its LaTeX command.
fn function_name(r: &[Node]) -> Option<String> {
    match r {
        [Node::Text(t)] if !t.style.normal => {
            let n = t.text.trim();
            symbols::FUNCTIONS.contains(&n).then(|| n.to_owned())
        }
        _ => None,
    }
}

/// Something taller than a line of text (it needs `\left…\right`).
fn tall(r: &[Node]) -> bool {
    r.iter().any(|n| match n {
        Node::Frac { kind, .. } => *kind != FracKind::Linear,
        Node::Matrix { .. } | Node::EqArray { .. } | Node::Nary { .. } => true,
        Node::Radical { body, .. } => tall(body),
        Node::Delim { items, .. } => items.iter().any(|i| tall(i)),
        Node::Scripts { base, .. } => tall(base),
        Node::Group { body } | Node::Boxed { body } => tall(body),
        Node::Limit { .. } | Node::GroupChr { .. } => true,
        _ => false,
    })
}

/// A big operator's operand needs braces when the reader would cut it
/// short: it holds a relation (or a + or −, for sums) at the top level.
fn needs_operand_braces(body: &[Node], stop_bin: bool) -> bool {
    if body.is_empty() {
        return true;
    }
    body.iter().any(|n| match n {
        Node::Text(t) if !t.style.normal => t.text.chars().any(|c| {
            symbols::class(c) == Class::Rel
                || c == '&'
                || (stop_bin && matches!(c, '+' | '-' | '−' | '±' | '∓'))
        }),
        _ => false,
    }) || matches!(body.first(), Some(Node::Group { .. }))
}
