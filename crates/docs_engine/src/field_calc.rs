//! Fields Word computes from other text: formulas (`= expression`, with
//! bookmarks, table cell references and `SUM(ABOVE)`), `IF`, `COMPARE`,
//! `STYLEREF` and `LISTNUM`, and Word's numeric pictures (`\#`).
//!
//! Formulas follow Word's: `+ - * / ^ %`, comparisons (`= < > <= >= <>`,
//! giving 1 or 0), `ABS AND AVERAGE COUNT DEFINED FALSE IF INT MAX MIN MOD
//! NOT OR PRODUCT ROUND SIGN SUM TRUE`; operands are numbers, bookmarks
//! (their text read as a number) and, in a table, cells (`A1`, `B2:C4`,
//! `ABOVE`, `BELOW`, `LEFT`, `RIGHT`). Errors are Word's texts
//! (`!Syntax Error, )`, `!Zero Divide`, `!Undefined Bookmark, Name`).

use std::collections::HashMap;

use crate::model::{OBJECT, Paragraph};
use crate::table::CellRef;

/// What a formula needs from the document.
pub(crate) struct FormulaEnv<'a> {
    pub paras: &'a [Paragraph],
    /// A bookmark's text by name (any case).
    pub bookmark: &'a dyn Fn(&str) -> Option<String>,
    /// The cell the field is in.
    pub cell: Option<CellRef>,
    /// The decimal sign (`.` or `,`).
    pub decimal: char,
}

/// A value: one number or a list (a range of cells).
#[derive(Clone, Debug)]
enum V {
    Num(f64),
    List(Vec<f64>),
}

impl V {
    fn num(&self) -> f64 {
        match self {
            V::Num(n) => *n,
            V::List(l) => l.first().copied().unwrap_or(0.0),
        }
    }

    fn flat(self, out: &mut Vec<f64>) {
        match self {
            V::Num(n) => out.push(n),
            V::List(l) => out.extend(l),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Num(f64),
    Name(String),
    Op(String),
    Open,
    Close,
    Comma,
}

fn tokens(src: &str, decimal: char) -> Result<Vec<Tok>, String> {
    let mut out = Vec::new();
    let cs: Vec<char> = src.chars().collect();
    let mut i = 0;
    while i < cs.len() {
        let c = cs[i];
        if c.is_whitespace() {
            i += 1;
        } else if c.is_ascii_digit()
            || (c == decimal && cs.get(i + 1).is_some_and(char::is_ascii_digit))
        {
            let start = i;
            while i < cs.len() && (cs[i].is_ascii_digit() || cs[i] == decimal) {
                i += 1;
            }
            let s: String = cs[start..i]
                .iter()
                .map(|&c| if c == decimal { '.' } else { c })
                .collect();
            out.push(Tok::Num(
                s.parse().map_err(|_| format!("!Syntax Error, {s}"))?,
            ));
        } else if c.is_alphabetic() || c == '_' {
            let start = i;
            while i < cs.len() && (cs[i].is_alphanumeric() || cs[i] == '_' || cs[i] == ':') {
                i += 1;
            }
            out.push(Tok::Name(cs[start..i].iter().collect()));
        } else {
            i += 1;
            out.push(match c {
                '(' => Tok::Open,
                ')' => Tok::Close,
                ',' | ';' => Tok::Comma,
                '<' | '>' if cs.get(i) == Some(&'=') => {
                    i += 1;
                    Tok::Op(format!("{c}="))
                }
                '<' if cs.get(i) == Some(&'>') => {
                    i += 1;
                    Tok::Op("<>".into())
                }
                '+' | '-' | '*' | '/' | '^' | '%' | '=' | '<' | '>' => Tok::Op(c.to_string()),
                // Word's own minus and multiplication signs.
                '−' => Tok::Op("-".into()),
                '×' => Tok::Op("*".into()),
                _ => return Err(format!("!Syntax Error, {c}")),
            });
        }
    }
    Ok(out)
}

struct Parser<'a, 'b> {
    toks: Vec<Tok>,
    at: usize,
    env: &'a FormulaEnv<'b>,
}

impl Parser<'_, '_> {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.at)
    }

    fn next(&mut self) -> Option<Tok> {
        let t = self.toks.get(self.at).cloned();
        self.at += 1;
        t
    }

    fn op(&self) -> Option<&str> {
        match self.peek() {
            Some(Tok::Op(o)) => Some(o),
            _ => None,
        }
    }

    fn syntax(&self) -> String {
        match self.peek() {
            Some(Tok::Close) => "!Syntax Error, )".into(),
            Some(Tok::Comma) => "!Syntax Error, ,".into(),
            Some(Tok::Op(o)) => format!("!Syntax Error, {o}"),
            Some(Tok::Num(n)) => format!("!Syntax Error, {n}"),
            Some(Tok::Name(n)) => format!("!Syntax Error, {n}"),
            Some(Tok::Open) => "!Syntax Error, (".into(),
            None => "!Syntax Error".into(),
        }
    }

    fn compare(&mut self) -> Result<V, String> {
        let a = self.add()?;
        if let Some(o) = self.op().map(str::to_owned)
            && matches!(o.as_str(), "=" | "<" | ">" | "<=" | ">=" | "<>")
        {
            self.at += 1;
            let b = self.add()?;
            let (x, y) = (a.num(), b.num());
            let r = match o.as_str() {
                "=" => x == y,
                "<" => x < y,
                ">" => x > y,
                "<=" => x <= y,
                ">=" => x >= y,
                _ => x != y,
            };
            return Ok(V::Num(r as u8 as f64));
        }
        Ok(a)
    }

    fn add(&mut self) -> Result<V, String> {
        let mut a = self.mul()?;
        while let Some(o) = self.op().map(str::to_owned)
            && (o == "+" || o == "-")
        {
            self.at += 1;
            let b = self.mul()?.num();
            a = V::Num(if o == "+" { a.num() + b } else { a.num() - b });
        }
        Ok(a)
    }

    fn mul(&mut self) -> Result<V, String> {
        let mut a = self.pow()?;
        while let Some(o) = self.op().map(str::to_owned)
            && (o == "*" || o == "/")
        {
            self.at += 1;
            let b = self.pow()?.num();
            a = V::Num(if o == "*" {
                a.num() * b
            } else if b == 0.0 {
                return Err("!Zero Divide".into());
            } else {
                a.num() / b
            });
        }
        Ok(a)
    }

    fn pow(&mut self) -> Result<V, String> {
        let mut a = self.unary()?;
        while self.op() == Some("^") {
            self.at += 1;
            let b = self.unary()?.num();
            a = V::Num(a.num().powf(b));
        }
        Ok(a)
    }

    fn unary(&mut self) -> Result<V, String> {
        match self.op() {
            Some("-") => {
                self.at += 1;
                Ok(V::Num(-self.unary()?.num()))
            }
            Some("+") => {
                self.at += 1;
                self.unary()
            }
            _ => {
                let v = self.primary()?;
                if self.op() == Some("%") {
                    self.at += 1;
                    return Ok(V::Num(v.num() / 100.0));
                }
                Ok(v)
            }
        }
    }

    fn primary(&mut self) -> Result<V, String> {
        match self.next() {
            Some(Tok::Num(n)) => Ok(V::Num(n)),
            Some(Tok::Open) => {
                let v = self.compare()?;
                if self.next() != Some(Tok::Close) {
                    self.at -= 1;
                    return Err(self.syntax());
                }
                Ok(v)
            }
            Some(Tok::Name(name)) => {
                if self.peek() == Some(&Tok::Open) {
                    self.at += 1;
                    return self.call(&name);
                }
                self.name(&name)
            }
            _ => {
                self.at -= 1;
                Err(self.syntax())
            }
        }
    }

    fn call(&mut self, f: &str) -> Result<V, String> {
        let upper = f.to_ascii_uppercase();
        let mut args: Vec<V> = Vec::new();
        if upper == "DEFINED" {
            // DEFINED(x): 1 when x works out, 0 when it's an error.
            let depth_start = self.at;
            let mut depth = 1;
            while let Some(t) = self.next() {
                match t {
                    Tok::Open => depth += 1,
                    Tok::Close => {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                    _ => {}
                }
            }
            let inner = self.toks[depth_start..self.at - 1].to_vec();
            let mut p = Parser {
                toks: inner,
                at: 0,
                env: self.env,
            };
            let ok = p.compare().is_ok() && p.at == p.toks.len();
            return Ok(V::Num(ok as u8 as f64));
        }
        if self.peek() != Some(&Tok::Close) {
            loop {
                args.push(self.compare()?);
                match self.next() {
                    Some(Tok::Comma) => continue,
                    Some(Tok::Close) => break,
                    _ => {
                        self.at -= 1;
                        return Err(self.syntax());
                    }
                }
            }
        } else {
            self.at += 1;
        }
        let list = |args: Vec<V>| {
            let mut out = Vec::new();
            for a in args {
                a.flat(&mut out);
            }
            out
        };
        let one = |args: &[V], k: usize| args.get(k).map_or(0.0, V::num);
        let r = match upper.as_str() {
            "SUM" => list(args).iter().sum(),
            "PRODUCT" => list(args).iter().product(),
            "AVERAGE" => {
                let l = list(args);
                if l.is_empty() {
                    0.0
                } else {
                    l.iter().sum::<f64>() / l.len() as f64
                }
            }
            "COUNT" => list(args).len() as f64,
            "MAX" => list(args).into_iter().fold(f64::NAN, f64::max),
            "MIN" => list(args).into_iter().fold(f64::NAN, f64::min),
            "AND" => list(args).iter().all(|&x| x != 0.0) as u8 as f64,
            "OR" => list(args).iter().any(|&x| x != 0.0) as u8 as f64,
            "NOT" => (one(&args, 0) == 0.0) as u8 as f64,
            "ABS" => one(&args, 0).abs(),
            "INT" => one(&args, 0).trunc(),
            "SIGN" => {
                let x = one(&args, 0);
                if x == 0.0 { 0.0 } else { x.signum() }
            }
            "MOD" => {
                let y = one(&args, 1);
                if y == 0.0 {
                    return Err("!Zero Divide".into());
                }
                one(&args, 0) % y
            }
            "ROUND" => {
                let k = 10f64.powi(one(&args, 1) as i32);
                let x = one(&args, 0) * k;
                // Word rounds halves away from zero.
                (x.abs() + 0.5).floor().copysign(x) / k
            }
            "IF" => {
                if one(&args, 0) != 0.0 {
                    one(&args, 1)
                } else {
                    one(&args, 2)
                }
            }
            "TRUE" => 1.0,
            "FALSE" => 0.0,
            _ => return Err(format!("!Syntax Error, {f}")),
        };
        Ok(V::Num(if r.is_nan() { 0.0 } else { r }))
    }

    /// A bare name: TRUE/FALSE, a direction, a cell or range, a bookmark.
    fn name(&mut self, name: &str) -> Result<V, String> {
        let upper = name.to_ascii_uppercase();
        match upper.as_str() {
            "TRUE" => return Ok(V::Num(1.0)),
            "FALSE" => return Ok(V::Num(0.0)),
            "ABOVE" | "BELOW" | "LEFT" | "RIGHT" => {
                return Ok(V::List(self.direction(&upper)));
            }
            _ => {}
        }
        if let Some(cells) = self.cells(&upper) {
            return Ok(cells);
        }
        match (self.env.bookmark)(name) {
            Some(text) => Ok(V::Num(number_in(&text, self.env.decimal).unwrap_or(0.0))),
            None => Err(format!("!Undefined Bookmark, {name}")),
        }
    }

    /// The table the field is in: each cell's text by (row, cell).
    fn grid(&self) -> HashMap<(u32, u32), String> {
        let mut out: HashMap<(u32, u32), String> = HashMap::new();
        let Some(here) = self.env.cell else {
            return out;
        };
        for p in self.env.paras {
            if let Some(c) = p.cell
                && c.table == here.table
            {
                let t = out.entry((c.row, c.cell)).or_default();
                if !t.is_empty() {
                    t.push(' ');
                }
                t.extend(p.text.chars().filter(|&c| c != OBJECT));
            }
        }
        out
    }

    fn direction(&self, dir: &str) -> Vec<f64> {
        let Some(here) = self.env.cell else {
            return Vec::new();
        };
        let grid = self.grid();
        let mut keys: Vec<&(u32, u32)> = grid
            .keys()
            .filter(|(r, c)| match dir {
                "ABOVE" => *c == here.cell && *r < here.row,
                "BELOW" => *c == here.cell && *r > here.row,
                "LEFT" => *r == here.row && *c < here.cell,
                _ => *r == here.row && *c > here.cell,
            })
            .collect();
        keys.sort();
        keys.into_iter()
            .filter_map(|k| number_in(&grid[k], self.env.decimal))
            .collect()
    }

    /// `A1` or `A1:C3` in the field's table.
    fn cells(&self, name: &str) -> Option<V> {
        let (a, b) = name.split_once(':').unwrap_or((name, name));
        let (r1, c1) = cell_name(a)?;
        let (r2, c2) = cell_name(b)?;
        self.env.cell?;
        let grid = self.grid();
        let value = |r: u32, c: u32| {
            grid.get(&(r, c))
                .and_then(|t| number_in(t, self.env.decimal))
        };
        if a == b {
            return Some(V::Num(value(r1, c1).unwrap_or(0.0)));
        }
        let mut out = Vec::new();
        for r in r1.min(r2)..=r1.max(r2) {
            for c in c1.min(c2)..=c1.max(c2) {
                out.extend(value(r, c));
            }
        }
        Some(V::List(out))
    }
}

/// `B3` → (row 2, cell 1).
fn cell_name(s: &str) -> Option<(u32, u32)> {
    let letters: String = s.chars().take_while(|c| c.is_ascii_alphabetic()).collect();
    let digits = &s[letters.len()..];
    if letters.is_empty() || letters.len() > 2 || digits.is_empty() {
        return None;
    }
    let row: u32 = digits.parse().ok()?;
    let col = letters.bytes().fold(0u32, |acc, b| {
        acc * 26 + (b.to_ascii_uppercase() - b'A' + 1) as u32
    });
    (row >= 1).then(|| (row - 1, col - 1))
}

/// The number written in `text` (`Rp 1,500.00`, `(12)`, `25%`), as Word
/// reads a cell or a bookmark.
pub(crate) fn number_in(text: &str, decimal: char) -> Option<f64> {
    let thousands = if decimal == ',' { '.' } else { ',' };
    let t = text.trim();
    let negative =
        t.starts_with('(') && t.ends_with(')') || t.starts_with('-') || t.starts_with('−');
    let mut s = String::new();
    let mut seen = false;
    for c in t.chars() {
        if c.is_ascii_digit() {
            s.push(c);
            seen = true;
        } else if c == decimal && !s.contains('.') {
            s.push('.');
        } else if c == thousands && seen {
        } else if seen && !c.is_whitespace() && c != '%' && c != ')' {
            break;
        }
    }
    let mut v: f64 = s.parse().ok()?;
    if negative {
        v = -v;
    }
    if t.ends_with('%') {
        v /= 100.0;
    }
    Some(v)
}

/// Work out a formula (`expr` without its `=`).
pub(crate) fn formula(expr: &str, env: &FormulaEnv) -> Result<f64, String> {
    let toks = tokens(expr, env.decimal)?;
    if toks.is_empty() {
        return Err("!Syntax Error".into());
    }
    let mut p = Parser { toks, at: 0, env };
    let v = p.compare()?;
    if p.at < p.toks.len() {
        return Err(p.syntax());
    }
    Ok(v.num())
}

/// A number as Word shows a formula's result: no picture gives up to two
/// decimals (`3.33`, `14`).
pub(crate) fn plain_number(v: f64, decimal: char) -> String {
    let r = (v * 100.0).round() / 100.0;
    let mut s = format!("{r:.2}");
    while s.contains('.') && (s.ends_with('0') || s.ends_with('.')) {
        s.pop();
    }
    if s == "-0" {
        s = "0".into();
    }
    if decimal != '.' {
        s = s.replace('.', &decimal.to_string());
    }
    s
}

/// `v` with a numeric picture (`\# "#,##0.00"`, `\# "$#,##0;($#,##0)"`,
/// `\# 0%`). `x` and `0` are digits always shown, `#` a digit or a space
/// (Word pads with spaces), `,` groups thousands, `.` is the decimal
/// sign; up to three sections: positive; negative; zero. Quoted text
/// (`'…'`) and other characters are written as they are.
///
/// `#` places with no digit are spaces in front of the number, as Word
/// writes them (Word 365 merges `\# "$#,##0.00"` of 150 as `$ 150.00`).
pub fn number_picture(v: f64, picture: &str, decimal: char) -> String {
    let sections: Vec<&str> = picture.split(';').collect();
    let (pic, value, sign) = if v < 0.0 && sections.len() > 1 {
        (sections[1], -v, false)
    } else if v == 0.0 && sections.len() > 2 {
        (sections[2], v, false)
    } else {
        (sections[0], v.abs(), v < 0.0)
    };
    // Where the digits are.
    let chars: Vec<char> = pic.chars().collect();
    let mut quoted = false;
    let mut first = None;
    let mut last = None;
    let mut point = None;
    for (i, &c) in chars.iter().enumerate() {
        if c == '\'' {
            quoted = !quoted;
            continue;
        }
        if quoted {
            continue;
        }
        if matches!(c, '0' | '#' | 'x') {
            first.get_or_insert(i);
            last = Some(i);
        } else if c == '.' && point.is_none() && first.is_some() {
            point = Some(i);
        }
    }
    let (Some(first), Some(last)) = (first, last) else {
        return pic.replace('\'', "");
    };
    let point = point.filter(|&p| p > first && p < last);
    let decimals = point.map_or(0, |p| {
        chars[p + 1..=last]
            .iter()
            .filter(|c| matches!(c, '0' | '#' | 'x'))
            .count()
    });
    let int_end = point.unwrap_or(last + 1);
    let int_digits = chars[first..int_end]
        .iter()
        .filter(|c| matches!(c, '0' | '#' | 'x'))
        .count();
    let grouping = chars[first..int_end].contains(&',');
    let percent = pic.contains('%');
    let value = if percent { value * 100.0 } else { value };
    let k = 10f64.powi(decimals as i32);
    let rounded = (value * k + 0.5).floor() / k;
    let whole = rounded.trunc() as u64;
    let frac = ((rounded - rounded.trunc()) * k).round() as u64;
    let mut int_s = whole.to_string();
    // Leading zeros as the picture asks.
    let min_int = chars[first..int_end]
        .iter()
        .skip_while(|&&c| c != '0')
        .filter(|&&c| c == '0')
        .count();
    if whole == 0 && min_int == 0 {
        int_s.clear();
    }
    while int_s.len() < min_int {
        int_s.insert(0, '0');
    }
    // `x` drops digits to its left.
    if let Some(xp) = chars[first..int_end].iter().position(|&c| c == 'x') {
        let keep = int_digits - xp;
        if int_s.len() > keep {
            int_s = int_s[int_s.len() - keep..].to_owned();
        }
    }
    // A `#` with no digit shows a space (`{ = 9 + 6 \# $### }` is
    // `$ 15`); separators between such places go.
    let mut pad = int_digits.saturating_sub(int_s.len());
    let thousands = if decimal == ',' { '.' } else { ',' };
    if grouping {
        let mut g = String::new();
        for (i, c) in int_s.chars().enumerate() {
            if i > 0 && (int_s.len() - i).is_multiple_of(3) {
                g.push(thousands);
            }
            g.push(c);
        }
        int_s = g;
    }
    let mut digits = int_s;
    if decimals > 0 {
        let mut f = format!("{frac:0decimals$}");
        // `#` after the point: trailing zeros dropped.
        let optional = point.map_or(0, |p| {
            chars[p + 1..=last]
                .iter()
                .rev()
                .take_while(|&&c| c == '#')
                .count()
        });
        for _ in 0..optional {
            if f.ends_with('0') {
                f.pop();
            }
        }
        if !f.is_empty() {
            digits.push(decimal);
            digits.push_str(&f);
        }
    }
    if digits.is_empty() {
        digits.push('0');
        pad = pad.saturating_sub(1);
    }
    let mut out = String::new();
    let mut quoted = false;
    let mut placed = false;
    for (i, &c) in chars.iter().enumerate() {
        if c == '\'' {
            quoted = !quoted;
            continue;
        }
        if !quoted && i >= first && i <= last {
            if !placed {
                out.extend(std::iter::repeat_n(' ', pad));
                if sign {
                    out.push('-');
                }
                out.push_str(&digits);
                placed = true;
            }
            continue;
        }
        if !quoted && c == '-' && i < first {
            // A minus in the picture is shown for negative values only.
            continue;
        }
        out.push(c);
    }
    out
}

/// How two values compare in `IF` and `COMPARE`: as numbers when both
/// are, else as text, `=`/`<>` with Word's `*` and `?` wildcards in the
/// second.
pub(crate) fn compare(a: &str, op: &str, b: &str, decimal: char) -> Option<bool> {
    let na = number_in(a, decimal).filter(|_| looks_numeric(a));
    let nb = number_in(b, decimal).filter(|_| looks_numeric(b));
    let ord = match (na, nb) {
        (Some(x), Some(y)) => x.partial_cmp(&y)?,
        _ => {
            if matches!(op, "=" | "<>") && (b.contains('*') || b.contains('?')) {
                let m = wildcard(a, b);
                return Some(if op == "=" { m } else { !m });
            }
            a.cmp(b)
        }
    };
    use std::cmp::Ordering::*;
    Some(match op {
        "=" => ord == Equal,
        "<>" => ord != Equal,
        "<" => ord == Less,
        ">" => ord == Greater,
        "<=" => ord != Greater,
        ">=" => ord != Less,
        _ => return None,
    })
}

fn looks_numeric(s: &str) -> bool {
    let t = s.trim();
    !t.is_empty()
        && t.chars()
            .all(|c| c.is_ascii_digit() || matches!(c, '.' | ',' | '-' | '+' | '%'))
}

fn wildcard(s: &str, pat: &str) -> bool {
    let s: Vec<char> = s.chars().collect();
    let p: Vec<char> = pat.chars().collect();
    fn go(s: &[char], p: &[char]) -> bool {
        match p.first() {
            None => s.is_empty(),
            Some('*') => (0..=s.len()).any(|k| go(&s[k..], &p[1..])),
            Some('?') => !s.is_empty() && go(&s[1..], &p[1..]),
            Some(c) => s.first() == Some(c) && go(&s[1..], &p[1..]),
        }
    }
    go(&s, &p)
}

/// `LISTNUM` numbering: Word's built-in lists' numbers per level.
pub(crate) fn listnum_label(list: &str, level: usize, n: u32) -> String {
    use crate::section::NumFormat::*;
    let list = list.to_ascii_lowercase();
    let level = level.clamp(1, 9);
    if list == "legaldefault" {
        return format!("{n}.");
    }
    let (fmt, open, close) = if list == "outlinedefault" {
        [
            (UpperRoman, "", "."),
            (UpperLetter, "", "."),
            (Decimal, "", "."),
            (LowerLetter, "", ")"),
            (Decimal, "(", ")"),
            (LowerLetter, "(", ")"),
            (LowerRoman, "(", ")"),
            (LowerLetter, "(", ")"),
            (LowerRoman, "(", ")"),
        ][level - 1]
    } else {
        [
            (Decimal, "", ")"),
            (LowerLetter, "", ")"),
            (LowerRoman, "", ")"),
            (Decimal, "(", ")"),
            (LowerLetter, "(", ")"),
            (LowerRoman, "(", ")"),
            (Decimal, "", "."),
            (LowerLetter, "", "."),
            (LowerRoman, "", "."),
        ][level - 1]
    };
    format!("{open}{}{close}", fmt.format(n))
}

/// A paragraph's visible text, spaces collapsed (what `STYLEREF` shows).
pub(crate) fn para_text(p: &Paragraph) -> String {
    let t: String = p.text.chars().filter(|&c| c != OBJECT).collect();
    t.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn eval(s: &str) -> Result<f64, String> {
        let none = |_: &str| None;
        let env = FormulaEnv {
            paras: &[],
            bookmark: &none,
            cell: None,
            decimal: '.',
        };
        formula(s, &env)
    }

    #[test]
    fn formulas_as_word_works_them_out() {
        assert_eq!(eval("2 + 3 * 4"), Ok(14.0));
        assert_eq!(eval("(2 + 3) * 4"), Ok(20.0));
        assert_eq!(eval("2^3^2"), Ok(64.0));
        assert_eq!(eval("-2 + 50%"), Ok(-1.5));
        assert_eq!(eval("ROUND(10/3, 2)"), Ok(3.33));
        assert_eq!(eval("ROUND(2.5, 0)"), Ok(3.0));
        assert_eq!(eval("MAX(3, 8, 5) - MIN(4, 2)"), Ok(6.0));
        assert_eq!(eval("AVERAGE(1, 2, 3, 6)"), Ok(3.0));
        assert_eq!(eval("IF(3 > 2, 10, 20)"), Ok(10.0));
        assert_eq!(eval("AND(1, 0) + OR(1, 0) + NOT(0)"), Ok(2.0));
        assert_eq!(eval("MOD(7, 3) + INT(2.7) + SIGN(-4) + ABS(-1)"), Ok(3.0));
        assert_eq!(eval("1 <> 2"), Ok(1.0));
        assert_eq!(eval("DEFINED(Missing) + DEFINED(1+1)"), Ok(1.0));
        assert_eq!(eval("1/0"), Err("!Zero Divide".into()));
        assert_eq!(eval("2 +"), Err("!Syntax Error".into()));
        assert_eq!(eval("(2"), Err("!Syntax Error".into()));
        assert_eq!(eval("2)"), Err("!Syntax Error, )".into()));
        assert_eq!(eval("Price * 2"), Err("!Undefined Bookmark, Price".into()));
        assert_eq!(plain_number(10.0 / 3.0, '.'), "3.33");
        assert_eq!(plain_number(14.0, '.'), "14");
        assert_eq!(plain_number(2.5, ','), "2,5");
    }

    #[test]
    fn numeric_pictures() {
        assert_eq!(number_picture(1234.5, "#,##0.00", '.'), "1,234.50");
        assert_eq!(number_picture(1234.5, "#,##0.00", ','), "1.234,50");
        assert_eq!(
            number_picture(-12.0, "$#,##0.00;($#,##0.00)", '.'),
            "($  12.00)"
        );
        // Word's own examples, and Word 365's mail merge of 150.
        assert_eq!(number_picture(15.0, "$###", '.'), "$ 15");
        assert_eq!(number_picture(9.0, "00.00", '.'), "09.00");
        assert_eq!(number_picture(150.0, "$#,##0.00", '.'), "$ 150.00");
        assert_eq!(number_picture(1234567.0, "$#,###,###", '.'), "$1,234,567");
        assert_eq!(number_picture(0.5, "#.00", '.'), " .50");
        assert_eq!(number_picture(0.0, "$###", '.'), "$  0");
        assert_eq!(number_picture(42.0, "$#,###,###", '.'), "$     42");
        assert_eq!(number_picture(0.0, "0;-0;'none'", '.'), "none");
        assert_eq!(number_picture(0.256, "0%", '.'), "26%");
        assert_eq!(number_picture(18.0, "0.00", '.'), "18.00");
        assert_eq!(number_picture(-5.0, "0", '.'), "-5");
        assert_eq!(number_picture(3.5, "0.##", '.'), "3.5");
        assert_eq!(number_picture(1234.0, "x##", '.'), "234");
        assert_eq!(number_picture(7.0, "'Total: '0", '.'), "Total: 7");
    }

    #[test]
    fn comparisons_and_wildcards() {
        assert_eq!(compare("4", ">", "3", '.'), Some(true));
        assert_eq!(compare("10", "<", "9", '.'), Some(false));
        assert_eq!(compare("abc", "=", "a*", '.'), Some(true));
        assert_eq!(compare("abc", "=", "a?d", '.'), Some(false));
        assert_eq!(compare("yes", "<>", "no", '.'), Some(true));
        assert_eq!(number_in("Rp 1,500.00", '.'), Some(1500.0));
        assert_eq!(number_in("1.500,25", ','), Some(1500.25));
        assert_eq!(number_in("(12)", '.'), Some(-12.0));
        assert_eq!(number_in("Item", '.'), None);
        assert_eq!(cell_name("B3"), Some((2, 1)));
        assert_eq!(cell_name("AA1"), Some((0, 26)));
        assert_eq!(listnum_label("NumberDefault", 2, 1), "a)");
        assert_eq!(listnum_label("OutlineDefault", 1, 4), "IV.");
    }
}
