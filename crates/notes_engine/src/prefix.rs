//! A line's container markup read as text (`> > ` quotes, indentation,
//! `- ` / `1. ` list markers, `[ ] ` task boxes), for the editing helpers:
//! Enter continues it on the next line, Tab indents it.
//!
//! This reads the line on its own, like a person would, rather than through
//! the parser: what Enter types must follow what the line *says*.

/// The markup at the start of a line.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct LinePrefix {
    /// `> ` markers (as written, spaces included), possibly empty.
    pub quote: String,
    /// Indentation after the quote markers.
    pub indent: String,
    pub list: Option<ListItem>,
    /// Bytes of the whole prefix: where the text starts.
    pub len: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListItem {
    /// `-`, `*`, `+` or a number with `.` / `)`.
    pub bullet: Bullet,
    /// Spaces after the bullet (at least one).
    pub gap: String,
    /// `Some(checked)` for a task item.
    pub task: Option<bool>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Bullet {
    Char(char),
    Number { n: u64, delim: char, width: usize },
}

impl Bullet {
    pub fn text(&self) -> String {
        match self {
            Self::Char(c) => c.to_string(),
            Self::Number { n, delim, .. } => format!("{n}{delim}"),
        }
    }

    /// The bullet of the item after this one.
    pub fn next(&self) -> Self {
        match *self {
            Self::Char(c) => Self::Char(c),
            Self::Number { n, delim, width } => Self::Number {
                n: n + 1,
                delim,
                width,
            },
        }
    }
}

impl LinePrefix {
    /// What a new line after this one starts with (Enter).
    pub fn continuation(&self) -> String {
        let mut s = self.quote.clone();
        s.push_str(&self.indent);
        if let Some(item) = &self.list {
            s.push_str(&item.bullet.next().text());
            s.push_str(&item.gap);
            if item.task.is_some() {
                s.push_str("[ ] ");
            }
        }
        s
    }

    /// Indentation that puts a nested item under this item's text.
    pub fn child_indent(&self) -> usize {
        match &self.list {
            Some(item) => item.bullet.text().len() + item.gap.len(),
            None => 0,
        }
    }
}

/// Read the markup at the start of `line`.
pub fn line_prefix(line: &str) -> LinePrefix {
    let b = line.as_bytes();
    let mut i = 0;
    // Quote markers: up to 3 spaces, `>`, one optional space; repeated.
    loop {
        let mut j = i;
        while j < b.len() && j - i < 3 && b[j] == b' ' {
            j += 1;
        }
        if b.get(j) != Some(&b'>') {
            break;
        }
        j += 1;
        if b.get(j) == Some(&b' ') {
            j += 1;
        }
        i = j;
    }
    let quote = line[..i].to_owned();
    let indent_start = i;
    while i < b.len() && (b[i] == b' ' || b[i] == b'\t') {
        i += 1;
    }
    let indent = line[indent_start..i].to_owned();
    let mut prefix = LinePrefix {
        quote,
        indent,
        list: None,
        len: i,
    };
    let rest = &line[i..];
    let bullet = match rest.as_bytes().first() {
        Some(&c @ (b'-' | b'*' | b'+')) => Some((Bullet::Char(c as char), 1)),
        Some(c) if c.is_ascii_digit() => {
            let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
            match rest.as_bytes().get(digits) {
                Some(&d @ (b'.' | b')')) if digits <= 9 => rest[..digits].parse().ok().map(|n| {
                    (
                        Bullet::Number {
                            n,
                            delim: d as char,
                            width: digits,
                        },
                        digits + 1,
                    )
                }),
                _ => None,
            }
        }
        _ => None,
    };
    let Some((bullet, blen)) = bullet else {
        return prefix;
    };
    let after = &rest[blen..];
    let gap_len = after.bytes().take_while(|&c| c == b' ').count();
    // `-` alone (an empty item) counts; `-x` or `---` don't.
    if gap_len == 0 && !after.is_empty() {
        return prefix;
    }
    let gap = if gap_len == 0 {
        " ".to_owned()
    } else {
        after[..gap_len].to_owned()
    };
    let mut len = i + blen + gap_len;
    let tail = &line[len..];
    let task = match tail.as_bytes() {
        [b'[', c @ (b' ' | b'x' | b'X'), b']', rest @ ..] if rest.is_empty() || rest[0] == b' ' => {
            len += 3 + usize::from(!rest.is_empty());
            Some(*c != b' ')
        }
        _ => None,
    };
    prefix.list = Some(ListItem { bullet, gap, task });
    prefix.len = len;
    prefix
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_prefixes() {
        let p = line_prefix("> > - [x] done");
        assert_eq!(p.quote, "> > ");
        assert_eq!(p.list.as_ref().unwrap().task, Some(true));
        assert_eq!(p.len, "> > - [x] ".len());
        assert_eq!(p.continuation(), "> > - [ ] ");

        let p = line_prefix("  9. nine");
        assert_eq!(p.indent, "  ");
        assert_eq!(p.continuation(), "  10. ");
        assert_eq!(p.child_indent(), 3);

        assert_eq!(line_prefix("---").list, None);
        assert_eq!(line_prefix("-x").list, None);
        assert!(line_prefix("-").list.is_some());
        assert_eq!(line_prefix("plain").len, 0);
        assert_eq!(line_prefix("> quote").continuation(), "> ");
    }
}
