//! VBA tokens (MS-VBAL §3.3, lexical tokens).
//!
//! - Line endings: CRLF (what the VBA editor saves), LF and CR are all accepted.
//! - ` _` at the end of a line continues the logical line (§3.2.2); the token after it isn't
//!   "first on the line".
//! - `'` and `Rem` start comments.
//! - Keywords are returned as [`Tok::Ident`]; the parser decides what is a keyword (after a `.`
//!   every word is a member name: `.Select`, `.End`).
//! - Numbers: decimal, `&H`/`&O` with VBA's typing (`&HFFFF` is the Integer -1, §3.3.2), type
//!   suffixes `% & ^ ! # @`; date literals `#1/2/2024#`; strings with `""` escapes.

use crate::object::{ScriptError, Value};

#[derive(Clone, Debug, PartialEq)]
pub enum Tok {
    /// A name or keyword, with its type-declaration character (`Left$`, `count&`).
    Ident(String, Option<char>),
    /// `[name]`: a foreign name.
    Foreign(String),
    /// A number, string or date literal.
    Lit(Value),
    Newline,
    Colon,
    Comma,
    Semi,
    Dot,
    Bang,
    LParen,
    RParen,
    Hash,
    /// `:=` (named argument).
    Assign,
    /// An operator: `+ - * / \ ^ & = < > <= >= <>`.
    Op(&'static str),
    Eof,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Token {
    pub tok: Tok,
    pub line: u32,
    /// Whitespace came right before this token (`Foo (x)` versus `Foo(x)`).
    pub space_before: bool,
    /// First token of a physical line (labels).
    pub line_start: bool,
}

fn is_ident_start(c: char) -> bool {
    c.is_alphabetic()
}

fn is_ident_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

struct Lexer {
    chars: Vec<char>,
    pos: usize,
    line: u32,
    out: Vec<Token>,
    space_before: bool,
    line_start: bool,
    stmt_start: bool,
}

/// Splits source into tokens. The result always ends with [`Tok::Eof`].
pub fn tokenize(src: &str) -> Result<Vec<Token>, ScriptError> {
    let mut lx = Lexer {
        chars: src.chars().collect(),
        pos: 0,
        line: 1,
        out: Vec::new(),
        space_before: false,
        line_start: true,
        stmt_start: true,
    };
    lx.run()?;
    Ok(lx.out)
}

impl Lexer {
    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn peek_at(&self, n: usize) -> Option<char> {
        self.chars.get(self.pos + n).copied()
    }

    fn err(&self, msg: impl Into<String>) -> ScriptError {
        let mut e = ScriptError::syntax(msg);
        e.location = Some(crate::object::Location {
            module: String::new(),
            procedure: String::new(),
            line: self.line,
        });
        e
    }

    fn push(&mut self, tok: Tok) {
        let stmt_end = matches!(tok, Tok::Newline | Tok::Colon);
        self.out.push(Token {
            tok,
            line: self.line,
            space_before: self.space_before,
            line_start: self.line_start,
        });
        self.space_before = false;
        self.line_start = false;
        self.stmt_start = stmt_end;
    }

    /// Consumes a line break at the cursor (CRLF, LF or CR).
    fn eat_newline(&mut self) -> bool {
        match self.peek() {
            Some('\r') => {
                self.pos += 1;
                if self.peek() == Some('\n') {
                    self.pos += 1;
                }
                true
            }
            Some('\n') => {
                self.pos += 1;
                true
            }
            _ => false,
        }
    }

    /// Whether the cursor is at `_` followed by blanks and a line break (a continuation).
    fn at_continuation(&self) -> bool {
        if self.peek() != Some('_') {
            return false;
        }
        let mut i = self.pos + 1;
        while let Some(c) = self.chars.get(i) {
            match c {
                ' ' | '\t' => i += 1,
                '\r' | '\n' => return true,
                _ => return false,
            }
        }
        true
    }

    fn skip_comment(&mut self) {
        // A comment line ending in " _" continues onto the next line (MS-VBAL §3.2.2).
        loop {
            let mut continued = false;
            while let Some(c) = self.peek() {
                if c == '\r' || c == '\n' {
                    break;
                }
                if c != ' ' && c != '\t' {
                    continued =
                        c == '_' && self.pos > 0 && matches!(self.chars[self.pos - 1], ' ' | '\t');
                }
                self.pos += 1;
            }
            if continued && self.peek().is_some() {
                self.eat_newline();
                self.line += 1;
                continue;
            }
            break;
        }
    }

    fn run(&mut self) -> Result<(), ScriptError> {
        while let Some(c) = self.peek() {
            match c {
                ' ' | '\t' | '\u{a0}' | '\u{feff}' => {
                    self.pos += 1;
                    self.space_before = true;
                }
                '\r' | '\n' => {
                    self.eat_newline();
                    self.push(Tok::Newline);
                    self.line += 1;
                    self.line_start = true;
                }
                '_' if self.at_continuation() => {
                    self.pos += 1;
                    while matches!(self.peek(), Some(' ' | '\t')) {
                        self.pos += 1;
                    }
                    self.eat_newline();
                    self.line += 1;
                    self.space_before = true;
                }
                '\'' => self.skip_comment(),
                '"' => self.string()?,
                '0'..='9' => self.number()?,
                '.' if self.peek_at(1).is_some_and(|d| d.is_ascii_digit())
                    && !self.follows_operand() =>
                {
                    self.number()?
                }
                '&' if matches!(self.peek_at(1), Some('h' | 'H' | 'o' | 'O'))
                    && self.peek_at(2).is_some_and(|d| d.is_ascii_hexdigit()) =>
                {
                    self.radix()?
                }
                '#' => self.hash(),
                '[' => {
                    let start = self.pos + 1;
                    let mut end = start;
                    while end < self.chars.len() && self.chars[end] != ']' {
                        if self.chars[end] == '\n' {
                            return Err(self.err("Expected ']'"));
                        }
                        end += 1;
                    }
                    if end >= self.chars.len() {
                        return Err(self.err("Expected ']'"));
                    }
                    let name: String = self.chars[start..end].iter().collect();
                    self.pos = end + 1;
                    self.push(Tok::Foreign(name));
                }
                ':' => {
                    if self.peek_at(1) == Some('=') {
                        self.pos += 2;
                        self.push(Tok::Assign);
                    } else {
                        self.pos += 1;
                        self.push(Tok::Colon);
                    }
                }
                ',' => self.single(Tok::Comma),
                ';' => self.single(Tok::Semi),
                '(' => self.single(Tok::LParen),
                ')' => self.single(Tok::RParen),
                '.' => self.single(Tok::Dot),
                '!' => self.single(Tok::Bang),
                '<' => match self.peek_at(1) {
                    Some('=') => self.op(2, "<="),
                    Some('>') => self.op(2, "<>"),
                    _ => self.op(1, "<"),
                },
                '>' => match self.peek_at(1) {
                    Some('=') => self.op(2, ">="),
                    _ => self.op(1, ">"),
                },
                '=' => match self.peek_at(1) {
                    // The editor rewrites these, but hand-written source may have them.
                    Some('<') => self.op(2, "<="),
                    Some('>') => self.op(2, ">="),
                    _ => self.op(1, "="),
                },
                '+' => self.op(1, "+"),
                '-' => self.op(1, "-"),
                '*' => self.op(1, "*"),
                '/' => self.op(1, "/"),
                '\\' => self.op(1, "\\"),
                '^' => self.op(1, "^"),
                '&' => self.op(1, "&"),
                c if is_ident_start(c) => self.ident(),
                c => return Err(self.err(format!("Invalid character '{c}'"))),
            }
        }
        if !matches!(self.out.last().map(|t| &t.tok), Some(Tok::Newline) | None) {
            self.push(Tok::Newline);
        }
        self.push(Tok::Eof);
        Ok(())
    }

    fn single(&mut self, tok: Tok) {
        self.pos += 1;
        self.push(tok);
    }

    fn op(&mut self, len: usize, op: &'static str) {
        self.pos += len;
        self.push(Tok::Op(op));
    }

    /// The previous token ends an operand and touches the cursor (`a.5` isn't a number).
    fn follows_operand(&self) -> bool {
        !self.space_before
            && matches!(
                self.out.last().map(|t| &t.tok),
                Some(Tok::Ident(..) | Tok::Foreign(_) | Tok::RParen | Tok::Lit(_))
            )
    }

    /// A type-declaration character at the cursor, when no name character follows it (so
    /// `rs!Field` is a bang and `a&b` a concatenation).
    fn suffix(&mut self, allowed: &str) -> Option<char> {
        let c = self.peek()?;
        if !allowed.contains(c) {
            return None;
        }
        if self
            .peek_at(1)
            .is_some_and(|n| is_ident_char(n) || n == '[')
        {
            return None;
        }
        self.pos += 1;
        Some(c)
    }

    fn ident(&mut self) {
        let start = self.pos;
        while self.peek().is_some_and(is_ident_char) {
            self.pos += 1;
        }
        let name: String = self.chars[start..self.pos].iter().collect();
        if self.stmt_start && name.eq_ignore_ascii_case("rem") {
            self.skip_comment();
            return;
        }
        let suffix = self.suffix("%&^!#@$");
        self.push(Tok::Ident(name, suffix));
    }

    fn string(&mut self) -> Result<(), ScriptError> {
        self.pos += 1;
        let mut s = String::new();
        loop {
            match self.peek() {
                None | Some('\r' | '\n') => return Err(self.err("Expected '\"'")),
                Some('"') => {
                    if self.peek_at(1) == Some('"') {
                        s.push('"');
                        self.pos += 2;
                    } else {
                        self.pos += 1;
                        break;
                    }
                }
                Some(c) => {
                    s.push(c);
                    self.pos += 1;
                }
            }
        }
        self.push(Tok::Lit(Value::String(s)));
        Ok(())
    }

    fn number(&mut self) -> Result<(), ScriptError> {
        let start = self.pos;
        let mut float = false;
        while self.peek().is_some_and(|c| c.is_ascii_digit()) {
            self.pos += 1;
        }
        if self.peek() == Some('.') && self.peek_at(1).is_some_and(|c| c.is_ascii_digit()) {
            float = true;
            self.pos += 1;
            while self.peek().is_some_and(|c| c.is_ascii_digit()) {
                self.pos += 1;
            }
        } else if self.peek() == Some('.') && !self.peek_at(1).is_some_and(is_ident_start) {
            // `1.` is a Double.
            float = true;
            self.pos += 1;
        }
        let mut text: String = self.chars[start..self.pos].iter().collect();
        if matches!(self.peek(), Some('e' | 'E' | 'd' | 'D')) {
            let sign = matches!(self.peek_at(1), Some('+' | '-'));
            let digit_at = if sign { 2 } else { 1 };
            if self.peek_at(digit_at).is_some_and(|c| c.is_ascii_digit()) {
                float = true;
                text.push('e');
                self.pos += 1;
                if sign {
                    text.push(self.peek().unwrap_or('+'));
                    self.pos += 1;
                }
                while let Some(c) = self.peek().filter(char::is_ascii_digit) {
                    text.push(c);
                    self.pos += 1;
                }
            }
        }
        let suffix = self.suffix("%&^!#@");
        let value = if float {
            let f: f64 = text.parse().map_err(|_| self.err("Invalid number"))?;
            match suffix {
                Some('!') => Value::Single(f as f32),
                Some('@') => Value::Currency((f * 10_000.0).round() as i64),
                _ => Value::Double(f),
            }
        } else {
            let f: f64 = text.parse().map_err(|_| self.err("Invalid number"))?;
            let n = text.parse::<i64>().ok();
            match (suffix, n) {
                (Some('%'), Some(n)) if i16::try_from(n).is_ok() => Value::Integer(n as i16),
                (Some('&'), Some(n)) if i32::try_from(n).is_ok() => Value::Long(n as i32),
                (Some('^'), Some(n)) => Value::LongLong(n),
                (Some('!'), _) => Value::Single(f as f32),
                (Some('#'), _) => Value::Double(f),
                (Some('@'), _) => Value::Currency((f * 10_000.0).round() as i64),
                (Some(_), _) => return Err(self.err("Overflow")),
                (None, Some(n)) if i16::try_from(n).is_ok() => Value::Integer(n as i16),
                (None, Some(n)) if i32::try_from(n).is_ok() => Value::Long(n as i32),
                // MS-VBAL §3.3.2: an integer literal too big for Long is a Double.
                (None, _) => Value::Double(f),
            }
        };
        self.push(Tok::Lit(value));
        Ok(())
    }

    fn radix(&mut self) -> Result<(), ScriptError> {
        let hex = matches!(self.peek_at(1), Some('h' | 'H'));
        self.pos += 2;
        let start = self.pos;
        while self.peek().is_some_and(|c| {
            if hex {
                c.is_ascii_hexdigit()
            } else {
                ('0'..='7').contains(&c)
            }
        }) {
            self.pos += 1;
        }
        let text: String = self.chars[start..self.pos].iter().collect();
        let v = u64::from_str_radix(&text, if hex { 16 } else { 8 })
            .map_err(|_| self.err("Overflow"))?;
        let suffix = self.suffix("%&^");
        // MS-VBAL §3.3.2: the literal's bits are reinterpreted in the smallest type that holds
        // them, so &HFFFF is the Integer -1 and &HFFFF& the Long 65535.
        let value = match suffix {
            Some('%') if v <= 0xFFFF => Value::Integer(v as u16 as i16),
            Some('&') if v <= 0xFFFF_FFFF => Value::Long(v as u32 as i32),
            Some('^') => Value::LongLong(v as i64),
            Some(_) => return Err(self.err("Overflow")),
            None if v <= 0xFFFF => Value::Integer(v as u16 as i16),
            None if v <= 0xFFFF_FFFF => Value::Long(v as u32 as i32),
            None => Value::LongLong(v as i64),
        };
        self.push(Tok::Lit(value));
        Ok(())
    }

    /// `#` starts a date literal when a valid date and a closing `#` follow on the line;
    /// otherwise it's a file-number marker (`Print #1`).
    fn hash(&mut self) {
        let mut end = self.pos + 1;
        while end < self.chars.len() && end - self.pos < 64 {
            match self.chars[end] {
                '#' => {
                    let text: String = self.chars[self.pos + 1..end].iter().collect();
                    if let Some(d) = super::convert::parse_date_literal(&text) {
                        self.pos = end + 1;
                        self.push(Tok::Lit(Value::Date(d)));
                        return;
                    }
                    break;
                }
                '\r' | '\n' => break,
                _ => end += 1,
            }
        }
        self.single(Tok::Hash);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toks(src: &str) -> Vec<Tok> {
        tokenize(src).unwrap().into_iter().map(|t| t.tok).collect()
    }

    #[test]
    fn literals_and_suffixes() {
        let t = toks("x = &HFFFF + &HFFFF& + 1.5 + 2# + 3% + 70000 + 1E3 + name$\r\n");
        assert!(t.contains(&Tok::Lit(Value::Integer(-1))));
        assert!(t.contains(&Tok::Lit(Value::Long(65535))));
        assert!(t.contains(&Tok::Lit(Value::Double(1.5))));
        assert!(t.contains(&Tok::Lit(Value::Double(2.0))));
        assert!(t.contains(&Tok::Lit(Value::Integer(3))));
        assert!(t.contains(&Tok::Lit(Value::Long(70000))));
        assert!(t.contains(&Tok::Lit(Value::Double(1000.0))));
        assert!(t.contains(&Tok::Ident("name".into(), Some('$'))));
    }

    #[test]
    fn continuation_comments_and_line_endings() {
        let src = "a = 1 + _\r\n    2 ' comment\r\nRem whole line\rb = \"say \"\"hi\"\"\"\n";
        let t = tokenize(src).unwrap();
        let kinds: Vec<_> = t.iter().map(|t| t.tok.clone()).collect();
        assert_eq!(
            kinds,
            vec![
                Tok::Ident("a".into(), None),
                Tok::Op("="),
                Tok::Lit(Value::Integer(1)),
                Tok::Op("+"),
                Tok::Lit(Value::Integer(2)),
                Tok::Newline,
                Tok::Newline,
                Tok::Ident("b".into(), None),
                Tok::Op("="),
                Tok::Lit(Value::String("say \"hi\"".into())),
                Tok::Newline,
                Tok::Eof,
            ]
        );
        assert_eq!(t[7].line, 4);
    }

    #[test]
    fn dates_hashes_and_bangs() {
        let t = toks("d = #1/2/2024#: Print #1, x: v = rs!Field: w = 5!\n");
        let d = crate::date::day_number(2024, 1, 2) as f64;
        assert!(t.contains(&Tok::Lit(Value::Date(d))));
        assert!(t.contains(&Tok::Hash));
        assert!(t.contains(&Tok::Bang));
        assert!(t.contains(&Tok::Lit(Value::Single(5.0))));
    }
}
