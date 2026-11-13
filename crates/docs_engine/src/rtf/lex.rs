//! RTF tokens: groups, control words and symbols, `\'hh` escapes, text and
//! `\binN` data. Never fails: malformed escapes are dropped, a truncated
//! file just ends.

/// One token of an RTF stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tok<'a> {
    Open,
    Close,
    /// `\word` or `\wordN` (the parameter clamped to `i32`).
    Word(&'a str, Option<i32>),
    /// `\` followed by one non-letter (`\~`, `\{`, `\*`, …).
    Symbol(u8),
    /// `\'hh`.
    Hex(u8),
    /// Plain bytes (line ends removed).
    Text(&'a [u8]),
    /// The data after `\binN`.
    Bin(&'a [u8]),
}

pub struct Lexer<'a> {
    s: &'a [u8],
    i: usize,
}

impl<'a> Lexer<'a> {
    pub fn new(s: &'a [u8]) -> Self {
        Self { s, i: 0 }
    }

    fn word(&mut self) -> Tok<'a> {
        let s = self.s;
        let start = self.i;
        while self.i < s.len() && s[self.i].is_ascii_alphabetic() && self.i - start < 32 {
            self.i += 1;
        }
        // Letters only, so this is always valid UTF-8.
        let name = std::str::from_utf8(&s[start..self.i]).unwrap_or("");
        let mut param = None;
        let neg = self.i + 1 < s.len() && s[self.i] == b'-' && s[self.i + 1].is_ascii_digit();
        if neg {
            self.i += 1;
        }
        if self.i < s.len() && s[self.i].is_ascii_digit() {
            let mut v: i64 = 0;
            let mut n = 0;
            while self.i < s.len() && s[self.i].is_ascii_digit() {
                if n < 12 {
                    v = v * 10 + i64::from(s[self.i] - b'0');
                }
                n += 1;
                self.i += 1;
            }
            if neg {
                v = -v;
            }
            param = Some(v.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32);
        }
        // One space ends a control word and is part of it.
        if self.i < s.len() && s[self.i] == b' ' {
            self.i += 1;
        }
        if name == "bin"
            && let Some(n) = param
        {
            let n = usize::try_from(n).unwrap_or(0).min(s.len() - self.i);
            let data = &s[self.i..self.i + n];
            self.i += n;
            return Tok::Bin(data);
        }
        Tok::Word(name, param)
    }
}

fn hex_digit(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

impl<'a> Iterator for Lexer<'a> {
    type Item = Tok<'a>;

    fn next(&mut self) -> Option<Tok<'a>> {
        let s = self.s;
        loop {
            let &b = s.get(self.i)?;
            match b {
                b'{' => {
                    self.i += 1;
                    return Some(Tok::Open);
                }
                b'}' => {
                    self.i += 1;
                    return Some(Tok::Close);
                }
                b'\r' | b'\n' => self.i += 1,
                b'\\' => {
                    self.i += 1;
                    let &c = s.get(self.i)?;
                    if c.is_ascii_alphabetic() {
                        return Some(self.word());
                    }
                    self.i += 1;
                    match c {
                        b'\'' => {
                            let h = s.get(self.i).copied().and_then(hex_digit);
                            let l = s.get(self.i + 1).copied().and_then(hex_digit);
                            if let (Some(h), Some(l)) = (h, l) {
                                self.i += 2;
                                return Some(Tok::Hex(h << 4 | l));
                            }
                            // A broken escape: drop it.
                        }
                        // `\` + line end is a paragraph mark.
                        b'\r' | b'\n' => return Some(Tok::Word("par", None)),
                        _ => return Some(Tok::Symbol(c)),
                    }
                }
                _ => {
                    let start = self.i;
                    while self.i < s.len() && !matches!(s[self.i], b'{' | b'}' | b'\\' | b'\r' | b'\n')
                    {
                        self.i += 1;
                    }
                    return Some(Tok::Text(&s[start..self.i]));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toks(s: &str) -> Vec<Tok<'_>> {
        Lexer::new(s.as_bytes()).collect()
    }

    #[std::prelude::v1::test]
    fn words_symbols_and_text() {
        assert_eq!(
            toks("{\\rtf1\\b0 Hi\\'e9\\~x}"),
            vec![
                Tok::Open,
                Tok::Word("rtf", Some(1)),
                Tok::Word("b", Some(0)),
                Tok::Text(b"Hi"),
                Tok::Hex(0xe9),
                Tok::Symbol(b'~'),
                Tok::Text(b"x"),
                Tok::Close,
            ]
        );
    }

    #[std::prelude::v1::test]
    fn negative_huge_params_and_bin() {
        assert_eq!(toks("\\fi-360"), vec![Tok::Word("fi", Some(-360))]);
        assert_eq!(
            toks("\\li99999999999999999"),
            vec![Tok::Word("li", Some(i32::MAX))]
        );
        assert_eq!(toks("\\bin3 a{}b"), vec![Tok::Bin(b"a{}"), Tok::Text(b"b")]);
        // Truncated: never past the end.
        assert_eq!(toks("\\bin99 ab"), vec![Tok::Bin(b"ab")]);
        assert_eq!(toks("x\\'4"), vec![Tok::Text(b"x"), Tok::Text(b"4")]);
        assert_eq!(toks("\\"), vec![]);
    }
}
