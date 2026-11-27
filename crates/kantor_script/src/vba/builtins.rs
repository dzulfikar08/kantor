//! The VBA standard library functions and constants macros use most (VBA language reference,
//! "Functions" and "Constants"). Functions that touch files, the registry, other programs or
//! the OS raise an "unsupported" error that names them.

use super::convert::{self, Scalar};
use super::format;
use super::interp::{Frame, Interp, R};
use crate::date;
use crate::object::{Arg, Array, ScriptError, Value, standard_message};

/// Whether `key` (lower case, without a type suffix) is a VBA library function.
pub fn is_builtin(key: &str) -> bool {
    matches!(
        key,
        "abs" | "array" | "asc" | "ascw" | "ascb" | "atn" | "beep" | "callbyname" | "cbool"
            | "cbyte" | "ccur" | "cdate" | "cvdate" | "cdbl" | "cdec" | "chr" | "chrw" | "chrb"
            | "choose" | "cint" | "clng" | "clnglng" | "clngptr" | "cos" | "createobject"
            | "csng" | "cstr" | "cvar" | "cverr" | "date" | "dateadd" | "datediff"
            | "datepart" | "dateserial" | "datevalue" | "day" | "doevents" | "err" | "erl"
            | "error" | "exp" | "filter" | "fix" | "format" | "formatcurrency"
            | "formatdatetime" | "formatnumber" | "formatpercent" | "hex" | "hour" | "iif"
            | "inputbox" | "instr" | "instrrev" | "int" | "isarray" | "isdate" | "isempty"
            | "iserror" | "ismissing" | "isnull" | "isnumeric" | "isobject" | "join" | "lbound"
            | "lcase" | "left" | "len" | "log" | "ltrim" | "mid" | "minute" | "month"
            | "monthname" | "msgbox" | "now" | "oct" | "randomize" | "replace" | "rgb"
            | "right" | "rnd" | "round" | "rtrim" | "second" | "sgn" | "sin" | "space"
            | "split" | "sqr" | "str" | "strcomp" | "strconv" | "string" | "strreverse"
            | "switch" | "tan" | "time" | "timer" | "timeserial" | "timevalue" | "trim"
            | "typename" | "ubound" | "ucase" | "val" | "vartype" | "weekday"
            | "weekdayname" | "year"
            // Unsupported: present so calls fail with a clear message.
            | "shell" | "dir" | "kill" | "filecopy" | "mkdir" | "rmdir" | "chdir" | "chdrive"
            | "curdir" | "filelen" | "filedatetime" | "getattr" | "setattr" | "freefile"
            | "eof" | "lof" | "loc" | "environ" | "sendkeys" | "appactivate" | "getobject"
            | "getsetting" | "savesetting" | "deletesetting" | "getallsettings" | "command"
            | "macscript" | "objptr" | "strptr" | "varptr" | "input" | "inputb"
    )
}

/// What an unsupported library function does, for its error message.
fn unsupported_feature(key: &str) -> Option<&'static str> {
    Some(match key {
        "shell" | "appactivate" | "sendkeys" | "macscript" => {
            "Running or controlling other programs"
        }
        "dir" | "kill" | "filecopy" | "mkdir" | "rmdir" | "chdir" | "chdrive" | "curdir"
        | "filelen" | "filedatetime" | "getattr" | "setattr" | "freefile" | "eof" | "lof"
        | "loc" | "input" | "inputb" => "File system access",
        "environ" | "command" => "Reading environment variables",
        "getobject" => "GetObject",
        "getsetting" | "savesetting" | "deletesetting" | "getallsettings" => "Registry settings",
        "objptr" | "strptr" | "varptr" => "Memory addresses (ObjPtr/StrPtr/VarPtr)",
        _ => return None,
    })
}

/// Parameter names, for named arguments (`MsgBox Prompt:="Hi", Buttons:=vbYesNo`).
pub fn params(key: &str) -> Option<&'static [&'static str]> {
    Some(match key {
        "msgbox" => &["prompt", "buttons", "title", "helpfile", "context"],
        "inputbox" => &[
            "prompt", "title", "default", "xpos", "ypos", "helpfile", "context",
        ],
        "format" => &["expression", "format", "firstdayofweek", "firstweekofyear"],
        "instr" => &["start", "string1", "string2", "compare"],
        "instrrev" => &["stringcheck", "stringmatch", "start", "compare"],
        "replace" => &["expression", "find", "replace", "start", "count", "compare"],
        "split" => &["expression", "delimiter", "limit", "compare"],
        "join" => &["sourcearray", "delimiter"],
        "filter" => &["sourcearray", "match", "include", "compare"],
        "strcomp" => &["string1", "string2", "compare"],
        "mid" => &["string", "start", "length"],
        "left" | "right" => &["string", "length"],
        "round" => &["number", "numdigitsafterdecimal"],
        "dateadd" => &["interval", "number", "date"],
        "datediff" => &[
            "interval",
            "date1",
            "date2",
            "firstdayofweek",
            "firstweekofyear",
        ],
        "datepart" => &["interval", "date", "firstdayofweek", "firstweekofyear"],
        "dateserial" => &["year", "month", "day"],
        "timeserial" => &["hour", "minute", "second"],
        "weekday" => &["date", "firstdayofweek"],
        "weekdayname" => &["weekday", "abbreviate", "firstdayofweek"],
        "monthname" => &["month", "abbreviate"],
        "formatnumber" | "formatpercent" | "formatcurrency" => &[
            "expression",
            "numdigitsafterdecimal",
            "includeleadingdigit",
            "useparensfornegativenumbers",
            "groupdigits",
        ],
        "formatdatetime" => &["date", "namedformat"],
        "iif" => &["expression", "truepart", "falsepart"],
        "lbound" | "ubound" => &["arrayname", "dimension"],
        "strconv" => &["string", "conversion", "lcid"],
        "string" => &["number", "character"],
        _ => return None,
    })
}

/// Functions whose arguments may be objects (others see the default member's value).
pub fn keeps_objects(key: &str) -> bool {
    matches!(
        key,
        "typename"
            | "isobject"
            | "ismissing"
            | "callbyname"
            | "array"
            | "iif"
            | "choose"
            | "switch"
            | "vartype"
            | "isempty"
            | "isnull"
            | "isnumeric"
            | "isdate"
            | "iserror"
            | "isarray"
    )
}

/// The VBA library's constants (`vbCrLf`, `vbYesNo`, `vbTextCompare`…).
pub fn constant(key: &str) -> Option<Value> {
    let long = |n: i32| Some(Value::Long(n));
    let text = |s: &str| Some(Value::String(s.to_string()));
    match key {
        "vbcrlf" | "vbnewline" => text("\r\n"),
        "vbcr" => text("\r"),
        "vblf" => text("\n"),
        "vbtab" => text("\t"),
        "vbnullstring" => text(""),
        "vbnullchar" => text("\0"),
        "vbback" => text("\u{8}"),
        "vbformfeed" => text("\u{c}"),
        "vbverticaltab" => text("\u{b}"),
        "vbobjecterror" => long(-2_147_221_504),
        "vbokonly"
        | "vbapplicationmodal"
        | "vbdefaultbutton1"
        | "vbbinarycompare"
        | "vbusesystemdayofweek"
        | "vbusesystem"
        | "vbgeneraldate"
        | "vbempty"
        | "vbfalse" => long(0),
        "vbokcancel" | "vbok" | "vbtextcompare" | "vbsunday" | "vbfirstjan1" | "vbnull"
        | "vbuppercase" | "vblongdate" | "vbmethod" => long(1),
        "vbabortretryignore" | "vbcancel" | "vbdatabasecompare" | "vbmonday"
        | "vbfirstfourdays" | "vbinteger" | "vblowercase" | "vbshortdate" | "vbget" => long(2),
        "vbyesnocancel" | "vbabort" | "vbtuesday" | "vbfirstfullweek" | "vblong"
        | "vbpropercase" | "vblongtime" => long(3),
        "vbyesno" | "vbretry" | "vbwednesday" | "vbsingle" | "vbshorttime" | "vblet" => long(4),
        "vbretrycancel" | "vbignore" | "vbthursday" | "vbdouble" => long(5),
        "vbyes" | "vbfriday" | "vbcurrency" => long(6),
        "vbno" | "vbsaturday" | "vbdate" => long(7),
        "vbstring" | "vbset" => long(8),
        "vbobject" => long(9),
        "vberror" => long(10),
        "vbboolean" => long(11),
        "vbvariant" => long(12),
        "vbdataobject" => long(13),
        "vbdecimal" => long(14),
        "vbcritical" => long(16),
        "vbbyte" => long(17),
        "vblonglong" => long(20),
        "vbquestion" => long(32),
        "vbuserdefinedtype" => long(36),
        "vbexclamation" => long(48),
        "vbinformation" => long(64),
        "vbdefaultbutton2" => long(256),
        "vbdefaultbutton3" => long(512),
        "vbdefaultbutton4" => long(768),
        "vbsystemmodal" => long(4096),
        "vbmsgboxhelpbutton" => long(16_384),
        "vbmsgboxsetforeground" => long(65_536),
        "vbmsgboxright" => long(524_288),
        "vbmsgboxrtlreading" => long(1_048_576),
        "vbarray" => long(8192),
        "vbtrue" => long(-1),
        "vbusedefault" => long(-2),
        "vbblack" => long(0),
        "vbred" => long(0xFF),
        "vbgreen" => long(0xFF00),
        "vbyellow" => long(0xFFFF),
        "vbblue" => long(0xFF_0000),
        "vbmagenta" => long(0xFF_00FF),
        "vbcyan" => long(0xFF_FF00),
        "vbwhite" => long(0xFF_FFFF),
        _ => None,
    }
}

/// Windows-1252 code points 128–159 (what `Chr` returns for them).
const CP1252: [u16; 32] = [
    0x20AC, 0x81, 0x201A, 0x0192, 0x201E, 0x2026, 0x2020, 0x2021, 0x02C6, 0x2030, 0x0160, 0x2039,
    0x0152, 0x8D, 0x017D, 0x8F, 0x90, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014,
    0x02DC, 0x2122, 0x0161, 0x203A, 0x0153, 0x9D, 0x017E, 0x0178,
];

struct Args(Vec<Value>);

impl Args {
    fn get(&self, i: usize) -> Option<&Value> {
        self.0.get(i).filter(|v| !v.is_missing())
    }

    fn req(&self, i: usize) -> R<&Value> {
        self.get(i).ok_or_else(|| ScriptError::runtime(449))
    }

    fn str(&self, i: usize) -> R<String> {
        convert::to_str(self.req(i)?)
    }

    fn f64(&self, i: usize) -> R<f64> {
        convert::to_f64(self.req(i)?)
    }

    fn i64(&self, i: usize) -> R<i64> {
        convert::to_i64(self.req(i)?)
    }

    fn i64_or(&self, i: usize, default: i64) -> R<i64> {
        match self.get(i) {
            Some(v) => convert::to_i64(v),
            None => Ok(default),
        }
    }

    fn str_or(&self, i: usize, default: &str) -> R<String> {
        match self.get(i) {
            Some(v) => convert::to_str(v),
            None => Ok(default.to_string()),
        }
    }

    fn date(&self, i: usize) -> R<f64> {
        convert::to_date(self.req(i)?)
    }

    fn is_null(&self, i: usize) -> bool {
        matches!(self.0.get(i), Some(Value::Null))
    }
}

fn invalid() -> ScriptError {
    ScriptError::runtime(5)
}

fn len_arg(n: i64) -> R<usize> {
    if n < 0 {
        Err(invalid())
    } else {
        Ok(n as usize)
    }
}

fn int_like(v: &Value, f: fn(f64) -> f64) -> R<Value> {
    Ok(match v {
        Value::Byte(_) | Value::Integer(_) | Value::Long(_) | Value::LongLong(_) => v.clone(),
        Value::Boolean(b) => Value::Integer(if *b { -1 } else { 0 }),
        Value::Single(x) => Value::Single(f(f64::from(*x)) as f32),
        Value::Currency(c) => Value::Currency((f(*c as f64 / 10_000.0) * 10_000.0) as i64),
        Value::Date(d) => Value::Date(f(*d)),
        other => Value::Double(f(convert::to_f64(other)?)),
    })
}

fn chars_of(s: &str) -> Vec<char> {
    s.chars().collect()
}

fn proper_case(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut start = true;
    for c in s.chars() {
        if c.is_alphanumeric() {
            if start {
                out.extend(c.to_uppercase());
            } else {
                out.extend(c.to_lowercase());
            }
            start = false;
        } else {
            out.push(c);
            start = true;
        }
    }
    out
}

/// Adds `n` intervals to a date (`DateAdd`).
fn date_add(interval: &str, n: f64, d: f64) -> R<f64> {
    let (day, secs) = date::split(d);
    let months = |m: i64| -> R<f64> {
        let (y, mo, dd) = date::ymd(d);
        let total = y * 12 + i64::from(mo) - 1 + m;
        let (ny, nm) = (total.div_euclid(12), (total.rem_euclid(12) + 1) as u32);
        if !(100..=9999).contains(&ny) {
            return Err(invalid());
        }
        let nd = dd.min(date::days_in_month(ny, nm));
        Ok(date::join(date::day_number(ny, nm, nd), f64::from(secs)))
    };
    let n_int = n.trunc() as i64;
    let r = match interval.to_ascii_lowercase().as_str() {
        "yyyy" => months(n_int * 12)?,
        "q" => months(n_int * 3)?,
        "m" => months(n_int)?,
        "y" | "d" | "w" => date::join(day + n_int, f64::from(secs)),
        "ww" => date::join(day + n_int * 7, f64::from(secs)),
        "h" | "n" | "s" => {
            let unit = match interval.to_ascii_lowercase().as_str() {
                "h" => 3600,
                "n" => 60,
                _ => 1,
            };
            let total = day * 86_400 + i64::from(secs) + n_int * unit;
            date::join(total.div_euclid(86_400), total.rem_euclid(86_400) as f64)
        }
        _ => return Err(invalid()),
    };
    if !(date::MIN..=date::MAX).contains(&r) {
        return Err(invalid());
    }
    Ok(r)
}

fn total_secs(d: f64) -> i64 {
    let (day, secs) = date::split(d);
    day * 86_400 + i64::from(secs)
}

/// `DateDiff`: counts interval boundaries crossed from `a` to `b`.
fn date_diff(interval: &str, a: f64, b: f64, first_day: u32) -> R<i64> {
    let (ya, ma, _) = date::ymd(a);
    let (yb, mb, _) = date::ymd(b);
    let (da, _) = date::split(a);
    let (db, _) = date::split(b);
    Ok(match interval.to_ascii_lowercase().as_str() {
        "yyyy" => yb - ya,
        "q" => (yb * 4 + i64::from((mb - 1) / 3)) - (ya * 4 + i64::from((ma - 1) / 3)),
        "m" => (yb * 12 + i64::from(mb)) - (ya * 12 + i64::from(ma)),
        "y" | "d" => db - da,
        "w" => (db - da) / 7,
        "ww" => {
            let start = |dn: i64| dn - (dn + 7 - i64::from(first_day)).rem_euclid(7);
            (start(db) - start(da)) / 7
        }
        "h" => total_secs(b).div_euclid(3600) - total_secs(a).div_euclid(3600),
        "n" => total_secs(b).div_euclid(60) - total_secs(a).div_euclid(60),
        "s" => total_secs(b) - total_secs(a),
        _ => return Err(invalid()),
    })
}

fn day_of_week(d: f64, first_day: u32) -> u32 {
    (date::weekday(d) + 7 - first_day) % 7 + 1
}

fn first_day(v: i64) -> R<u32> {
    match v {
        0 => Ok(1),
        1..=7 => Ok(v as u32),
        _ => Err(invalid()),
    }
}

fn hex_oct(v: &Value, radix: u32) -> R<Value> {
    if v.is_null() {
        return Ok(Value::Null);
    }
    let n = convert::to_i64(v)?;
    let bits: u64 = match v {
        Value::Byte(_) | Value::Integer(_) | Value::Boolean(_) | Value::Empty => {
            (n as i16) as u16 as u64
        }
        Value::LongLong(_) => n as u64,
        _ => {
            if i32::try_from(n).is_err() {
                return Err(ScriptError::runtime(6));
            }
            (n as i32) as u32 as u64
        }
    };
    Ok(Value::String(if radix == 16 {
        format!("{bits:X}")
    } else {
        format!("{bits:o}")
    }))
}

impl Interp<'_> {
    fn rnd(&mut self, n: Option<f64>) -> f32 {
        // VBA's generator (24-bit LCG): x = (x * &H43FD43FD + &HC39EC3) Mod 2^24, starting at
        // &H50000, so the first Rnd is 0.7055475 as in Office.
        let mut seed = self.st.rnd_seed;
        match n {
            Some(x) if x == 0.0 => return seed as f32 / 16_777_216.0,
            Some(x) if x < 0.0 => {
                let bits = u64::from((x as f32).to_bits());
                seed = ((bits + (bits >> 24)) & 0xFF_FFFF) as u32;
            }
            _ => {}
        }
        seed = ((u64::from(seed) * 0x43FD_43FD + 0xC3_9EC3) & 0xFF_FFFF) as u32;
        self.st.rnd_seed = seed;
        seed as f32 / 16_777_216.0
    }

    fn randomize(&mut self, n: f64) {
        let hi = (n.to_bits() >> 32) as u32 as i32;
        let mixed = ((hi & 0xFFFF) ^ (hi >> 16)) << 8;
        self.st.rnd_seed = ((self.st.rnd_seed & 0xFF00_00FF) | (mixed as u32)) & 0xFF_FFFF;
    }

    fn timer(&mut self) -> f64 {
        let now = self.host.now();
        (now - now.floor()) * 86_400.0
    }

    pub(crate) fn builtin(&mut self, key: &str, args: Vec<Value>, f: &mut Frame) -> R<Value> {
        if let Some(feature) = unsupported_feature(key) {
            let name = format!("{feature} ({})", title_case(key));
            return Err(ScriptError::unsupported(&name));
        }
        let a = Args(args);
        let text = f.module.compare_text;
        let compare = |a: &Args, i: usize| -> R<bool> {
            Ok(match a.get(i) {
                Some(v) => convert::to_i64(v)? == 1,
                None => text,
            })
        };
        if matches!(
            key,
            "len"
                | "left"
                | "right"
                | "mid"
                | "ucase"
                | "lcase"
                | "trim"
                | "ltrim"
                | "rtrim"
                | "abs"
                | "int"
                | "fix"
                | "year"
                | "month"
                | "day"
                | "hour"
                | "minute"
                | "second"
                | "weekday"
                | "str"
                | "strreverse"
                | "space"
                | "datevalue"
                | "timevalue"
        ) && a.is_null(0)
        {
            return Ok(Value::Null);
        }
        Ok(match key {
            // ---- Strings ----
            "len" => Value::Long(a.str(0)?.chars().count() as i32),
            "left" => {
                let n = len_arg(a.i64(1)?)?;
                Value::String(a.str(0)?.chars().take(n).collect())
            }
            "right" => {
                let n = len_arg(a.i64(1)?)?;
                let c = chars_of(&a.str(0)?);
                Value::String(c[c.len().saturating_sub(n)..].iter().collect())
            }
            "mid" => {
                let c = chars_of(&a.str(0)?);
                let start = a.i64(1)?;
                if start < 1 {
                    return Err(invalid());
                }
                let from = (start as usize - 1).min(c.len());
                let n = match a.get(2) {
                    Some(v) => len_arg(convert::to_i64(v)?)?,
                    None => c.len(),
                };
                Value::String(c[from..].iter().take(n).collect())
            }
            "instr" => {
                let (start, s1, s2, cmp) = if a.0.len() >= 3 {
                    (a.i64(0)?, 1, 2, 3)
                } else {
                    (1, 0, 1, 99)
                };
                if a.is_null(s1) || a.is_null(s2) {
                    return Ok(Value::Null);
                }
                if start < 1 {
                    return Err(invalid());
                }
                let hay = a.str(s1)?;
                let needle = a.str(s2)?;
                let text = if cmp == 99 { text } else { compare(&a, cmp)? };
                let from = start as usize - 1;
                if from > hay.chars().count() {
                    return Ok(Value::Long(0));
                }
                Value::Long(
                    convert::find_str(&hay, &needle, from, text).map_or(0, |i| i as i32 + 1),
                )
            }
            "instrrev" => {
                if a.is_null(0) || a.is_null(1) {
                    return Ok(Value::Null);
                }
                let hay = a.str(0)?;
                let needle = a.str(1)?;
                let len = hay.chars().count();
                let start = a.i64_or(2, -1)?;
                if start == 0 || start < -1 {
                    return Err(invalid());
                }
                let end = if start == -1 { len } else { start as usize };
                if needle.is_empty() {
                    return Ok(Value::Long(if end > len { 0 } else { end as i32 }));
                }
                if end > len {
                    return Ok(Value::Long(0));
                }
                Value::Long(
                    convert::rfind_str(&hay, &needle, end, compare(&a, 3)?)
                        .map_or(0, |i| i as i32 + 1),
                )
            }
            "replace" => {
                if a.is_null(0) {
                    return Ok(Value::Null);
                }
                let s = chars_of(&a.str(0)?);
                let find = a.str(1)?;
                let repl = a.str(2)?;
                let start = a.i64_or(3, 1)?;
                let count = a.i64_or(4, -1)?;
                if start < 1 || count < -1 {
                    return Err(invalid());
                }
                // VBA returns the string from `start` on, with replacements.
                let from = (start as usize - 1).min(s.len());
                let rest: String = s[from..].iter().collect();
                if find.is_empty() || count == 0 {
                    return Ok(Value::String(rest));
                }
                let text = compare(&a, 5)?;
                let mut out = String::new();
                let mut pos = 0usize;
                let rest_len = rest.chars().count();
                let rest_chars = chars_of(&rest);
                let find_len = find.chars().count();
                let mut done = 0i64;
                while pos <= rest_len {
                    if count != -1 && done >= count {
                        break;
                    }
                    match convert::find_str(&rest, &find, pos, text) {
                        Some(i) => {
                            out.extend(&rest_chars[pos..i]);
                            out.push_str(&repl);
                            pos = i + find_len;
                            done += 1;
                        }
                        None => break,
                    }
                }
                out.extend(&rest_chars[pos.min(rest_len)..]);
                Value::String(out)
            }
            "split" => {
                let s = a.str(0)?;
                let delim = a.str_or(1, " ")?;
                let limit = a.i64_or(2, -1)?;
                let text = compare(&a, 3)?;
                let mut parts: Vec<Value> = Vec::new();
                if !s.is_empty() {
                    if delim.is_empty() || limit == 1 {
                        parts.push(Value::String(s));
                    } else {
                        let chars = chars_of(&s);
                        let dl = delim.chars().count();
                        let mut pos = 0;
                        while let Some(i) = convert::find_str(&s, &delim, pos, text) {
                            if limit > 0 && parts.len() as i64 == limit - 1 {
                                break;
                            }
                            parts.push(Value::String(chars[pos..i].iter().collect()));
                            pos = i + dl;
                        }
                        parts.push(Value::String(chars[pos..].iter().collect()));
                    }
                }
                Value::array(Array::from_vec(0, parts))
            }
            "join" => {
                let Value::Array(arr) = a.req(0)? else {
                    return Err(ScriptError::runtime(13));
                };
                if arr.rank() > 1 {
                    return Err(invalid());
                }
                let delim = a.str_or(1, " ")?;
                let mut out = Vec::with_capacity(arr.len());
                for v in arr.values() {
                    out.push(if v.is_null() {
                        String::new()
                    } else {
                        convert::to_str(v)?
                    });
                }
                Value::String(out.join(&delim))
            }
            "filter" => {
                let Value::Array(arr) = a.req(0)? else {
                    return Err(ScriptError::runtime(13));
                };
                let m = a.str(1)?;
                let include = match a.get(2) {
                    Some(v) => convert::to_bool(v)?,
                    None => true,
                };
                let text = compare(&a, 3)?;
                let mut out = Vec::new();
                for v in arr.values() {
                    let s = convert::to_str(v)?;
                    if convert::find_str(&s, &m, 0, text).is_some() == include {
                        out.push(Value::String(s));
                    }
                }
                Value::array(Array::from_vec(0, out))
            }
            "trim" => Value::String(a.str(0)?.trim_matches(' ').to_string()),
            "ltrim" => Value::String(a.str(0)?.trim_start_matches(' ').to_string()),
            "rtrim" => Value::String(a.str(0)?.trim_end_matches(' ').to_string()),
            "ucase" => Value::String(a.str(0)?.to_uppercase()),
            "lcase" => Value::String(a.str(0)?.to_lowercase()),
            "strcomp" => {
                if a.is_null(0) || a.is_null(1) {
                    return Ok(Value::Null);
                }
                let o = convert::cmp_str(&a.str(0)?, &a.str(1)?, compare(&a, 2)?);
                Value::Integer(o as i16)
            }
            "string" => {
                let n = len_arg(a.i64(0)?)?;
                let c = match a.req(1)? {
                    Value::String(s) => s.chars().next().ok_or_else(invalid)?,
                    Value::Null => return Ok(Value::Null),
                    other => {
                        let code = convert::to_i64(other)?.rem_euclid(256) as u32;
                        char::from_u32(code).unwrap_or('?')
                    }
                };
                Value::String(std::iter::repeat_n(c, n).collect())
            }
            "space" => Value::String(" ".repeat(len_arg(a.i64(0)?)?)),
            "chr" | "chrb" => {
                let n = a.i64(0)?;
                let code = match n {
                    0..=127 | 160..=255 => n as u32,
                    128..=159 => u32::from(CP1252[n as usize - 128]),
                    _ => return Err(invalid()),
                };
                Value::String(char::from_u32(code).unwrap_or('?').to_string())
            }
            "chrw" => {
                let mut n = a.i64(0)?;
                if n < 0 {
                    n += 65_536;
                }
                if !(0..=0xFFFF).contains(&n) {
                    return Err(invalid());
                }
                Value::String(char::from_u32(n as u32).unwrap_or('\u{fffd}').to_string())
            }
            "asc" | "ascb" => {
                let s = a.str(0)?;
                let c = s.chars().next().ok_or_else(invalid)?;
                let code = c as u32;
                let n = if code < 128 || (160..256).contains(&code) {
                    code as i32
                } else if let Some(i) = CP1252.iter().position(|x| u32::from(*x) == code) {
                    128 + i as i32
                } else {
                    63
                };
                Value::Integer(n as i16)
            }
            "ascw" => {
                let s = a.str(0)?;
                let c = s.chars().next().ok_or_else(invalid)?;
                let mut buf = [0u16; 2];
                Value::Integer(c.encode_utf16(&mut buf)[0] as i16)
            }
            "strreverse" => Value::String(a.str(0)?.chars().rev().collect()),
            "strconv" => {
                let s = a.str(0)?;
                match a.i64(1)? {
                    1 => Value::String(s.to_uppercase()),
                    2 => Value::String(s.to_lowercase()),
                    3 => Value::String(proper_case(&s)),
                    64 | 128 => Value::String(s),
                    _ => return Err(invalid()),
                }
            }
            "format" => {
                let v = a.0.first().cloned().unwrap_or(Value::Empty);
                let v = if v.is_missing() { Value::Empty } else { v };
                let fmt = a.str_or(1, "")?;
                let fd = first_day(a.i64_or(2, 1)?)?;
                let fw = a.i64_or(3, 1)?.clamp(1, 3) as u32;
                Value::String(format::format(&v, &fmt, fd, fw)?)
            }
            "formatnumber" | "formatpercent" | "formatcurrency" => {
                if a.is_null(0) {
                    return Ok(Value::Null);
                }
                let mut x = a.f64(0)?;
                if key == "formatpercent" {
                    x *= 100.0;
                }
                let digits = a.i64_or(1, -1)? as i32;
                let lead = a.i64_or(2, -2)? != 0;
                let parens = a.i64_or(3, -2)?;
                let parens = if parens == -2 {
                    key == "formatcurrency"
                } else {
                    parens != 0
                };
                let group = a.i64_or(4, -2)? != 0;
                let body = format::format_fixed(x, digits, lead, parens, group);
                Value::String(match key {
                    "formatpercent" => format!("{body}%"),
                    "formatcurrency" => {
                        if let Some(inner) = body.strip_prefix('(') {
                            format!("(${inner}")
                        } else if let Some(inner) = body.strip_prefix('-') {
                            format!("-${inner}")
                        } else {
                            format!("${body}")
                        }
                    }
                    _ => body,
                })
            }
            "formatdatetime" => {
                if a.is_null(0) {
                    return Ok(Value::Null);
                }
                let d = Value::Date(a.date(0)?);
                let fmt = match a.i64_or(1, 0)? {
                    0 => "General Date",
                    1 => "Long Date",
                    2 => "Short Date",
                    3 => "Long Time",
                    4 => "Short Time",
                    _ => return Err(invalid()),
                };
                Value::String(format::format(&d, fmt, 1, 1)?)
            }

            // ---- Conversion ----
            "cstr" => Value::String(a.str(0)?),
            "cbool" => convert::convert(a.req(0)?, Scalar::Boolean)?,
            "cbyte" => convert::convert(a.req(0)?, Scalar::Byte)?,
            "cint" => convert::convert(a.req(0)?, Scalar::Integer)?,
            "clng" => convert::convert(a.req(0)?, Scalar::Long)?,
            "clnglng" | "clngptr" => convert::convert(a.req(0)?, Scalar::LongLong)?,
            "csng" => convert::convert(a.req(0)?, Scalar::Single)?,
            "cdbl" | "cdec" => convert::convert(a.req(0)?, Scalar::Double)?,
            "ccur" => convert::convert(a.req(0)?, Scalar::Currency)?,
            "cdate" | "cvdate" => {
                if key == "cvdate" && a.is_null(0) {
                    return Ok(Value::Null);
                }
                convert::convert(a.req(0)?, Scalar::Date)?
            }
            "cvar" => a.req(0)?.clone(),
            "cverr" => Value::Error(a.i64(0)? as i32),
            "val" => Value::Double(convert::val(&a.str(0)?)),
            "str" => {
                let v = a.req(0)?;
                let s = convert::to_str(v)?;
                if convert::to_f64(v)? >= 0.0 {
                    Value::String(format!(" {s}"))
                } else {
                    Value::String(s)
                }
            }
            "hex" => hex_oct(a.req(0)?, 16)?,
            "oct" => hex_oct(a.req(0)?, 8)?,
            "int" => int_like(a.req(0)?, f64::floor)?,
            "fix" => int_like(a.req(0)?, f64::trunc)?,
            "round" => {
                let v = a.req(0)?;
                if v.is_null() {
                    return Ok(Value::Null);
                }
                let n = a.i64_or(1, 0)?;
                if n < 0 {
                    return Err(invalid());
                }
                let x = convert::to_f64(v)?;
                let m = 10f64.powi(n as i32);
                // VBA's Round uses banker's rounding.
                let r = (x * m).round_ties_even() / m;
                match v {
                    Value::Integer(_) | Value::Long(_) | Value::Byte(_) | Value::LongLong(_) => {
                        v.clone()
                    }
                    Value::Single(_) => Value::Single(r as f32),
                    Value::Currency(_) => convert::convert(&Value::Double(r), Scalar::Currency)?,
                    _ => Value::Double(r),
                }
            }

            // ---- Information ----
            "isempty" => Value::Boolean(match a.req(0)? {
                Value::Empty => true,
                Value::Object(Some(o)) => {
                    let o = o.clone();
                    matches!(
                        self.obj_get(&o, "", Vec::new(), super::interp::Access::Get),
                        Ok(Value::Empty)
                    )
                }
                _ => false,
            }),
            "isnull" => Value::Boolean(matches!(self.plain(a.req(0)?.clone()), Value::Null)),
            "isnumeric" => {
                let v = self.plain(a.req(0)?.clone());
                Value::Boolean(match &v {
                    Value::Empty | Value::Boolean(_) => true,
                    Value::String(s) => convert::parse_number(s).is_some(),
                    other => other.is_number(),
                })
            }
            "isdate" => {
                let v = self.plain(a.req(0)?.clone());
                Value::Boolean(match &v {
                    Value::Date(_) => true,
                    Value::String(s) => convert::parse_date(s).is_some(),
                    _ => false,
                })
            }
            "isarray" => Value::Boolean(matches!(a.req(0)?, Value::Array(_))),
            "isobject" => Value::Boolean(matches!(a.req(0)?, Value::Object(_))),
            "ismissing" => Value::Boolean(matches!(a.0.first(), None | Some(Value::Missing))),
            "iserror" => {
                let v = self.plain(a.req(0)?.clone());
                Value::Boolean(matches!(v, Value::Error(_) | Value::Missing))
            }
            "typename" => Value::String(convert::type_name(a.req(0)?)),
            "vartype" => {
                let v = a.req(0)?.clone();
                let v = match v {
                    Value::Object(Some(_)) => self.deref(v.clone()).unwrap_or(v),
                    other => other,
                };
                Value::Integer(convert::var_type(&v) as i16)
            }
            "lbound" | "ubound" => {
                let Value::Array(arr) = a.req(0)? else {
                    return Err(ScriptError::runtime(13));
                };
                let dim = a.i64_or(1, 1)?;
                if dim < 1 || dim as usize > arr.rank() {
                    return Err(ScriptError::runtime(9));
                }
                let d = arr.dims()[dim as usize - 1];
                Value::Long(if key == "lbound" { d.lower } else { d.upper() })
            }

            // ---- Dates ----
            "now" => Value::Date(self.host.now()),
            "date" => Value::Date(self.host.now().floor()),
            "time" => {
                let now = self.host.now();
                Value::Date(now - now.floor())
            }
            "timer" => Value::Single(self.timer() as f32),
            "dateserial" => {
                let y = a.i64(0)?;
                let y = if (0..100).contains(&y) {
                    convert::expand_year(y, 2)
                } else {
                    y
                };
                let day = date::day_number_normalised(y, a.i64(1)?, a.i64(2)?);
                let d = day as f64;
                if !(date::MIN..=date::MAX).contains(&d) {
                    return Err(invalid());
                }
                Value::Date(d)
            }
            "timeserial" => {
                let total = a.i64(0)? * 3600 + a.i64(1)? * 60 + a.i64(2)?;
                Value::Date(date::join(
                    total.div_euclid(86_400),
                    total.rem_euclid(86_400) as f64,
                ))
            }
            "datevalue" => {
                let d = a.date(0)?;
                Value::Date(date::split(d).0 as f64)
            }
            "timevalue" => {
                let d = a.date(0)?;
                Value::Date(f64::from(date::split(d).1) / 86_400.0)
            }
            "dateadd" => {
                if a.is_null(2) {
                    return Ok(Value::Null);
                }
                Value::Date(date_add(&a.str(0)?, a.f64(1)?, a.date(2)?)?)
            }
            "datediff" => {
                if a.is_null(1) || a.is_null(2) {
                    return Ok(Value::Null);
                }
                let fd = first_day(a.i64_or(3, 1)?)?;
                let n = date_diff(&a.str(0)?, a.date(1)?, a.date(2)?, fd)?;
                i32::try_from(n).map_or(Value::LongLong(n), Value::Long)
            }
            "datepart" => {
                if a.is_null(1) {
                    return Ok(Value::Null);
                }
                let d = a.date(1)?;
                let fd = first_day(a.i64_or(2, 1)?)?;
                let fw = a.i64_or(3, 1)?.clamp(1, 3) as u32;
                let (y, m, dd) = date::ymd(d);
                let (h, mi, s) = date::hms(d);
                let n: i64 = match a.str(0)?.to_ascii_lowercase().as_str() {
                    "yyyy" => y,
                    "q" => i64::from((m - 1) / 3 + 1),
                    "m" => i64::from(m),
                    "y" => date::split(d).0 - date::day_number(y, 1, 1) + 1,
                    "d" => i64::from(dd),
                    "w" => i64::from(day_of_week(d, fd)),
                    "ww" => format::week_of_year(d, fd, fw),
                    "h" => i64::from(h),
                    "n" => i64::from(mi),
                    "s" => i64::from(s),
                    _ => return Err(invalid()),
                };
                Value::Integer(n as i16)
            }
            "year" => Value::Integer(date::ymd(a.date(0)?).0 as i16),
            "month" => Value::Integer(date::ymd(a.date(0)?).1 as i16),
            "day" => Value::Integer(date::ymd(a.date(0)?).2 as i16),
            "hour" => Value::Integer(date::hms(a.date(0)?).0 as i16),
            "minute" => Value::Integer(date::hms(a.date(0)?).1 as i16),
            "second" => Value::Integer(date::hms(a.date(0)?).2 as i16),
            "weekday" => {
                let fd = first_day(a.i64_or(1, 1)?)?;
                Value::Integer(day_of_week(a.date(0)?, fd) as i16)
            }
            "weekdayname" => {
                let n = a.i64(0)?;
                if !(1..=7).contains(&n) {
                    return Err(invalid());
                }
                let abbrev = match a.get(1) {
                    Some(v) => convert::to_bool(v)?,
                    None => false,
                };
                let fd = i64::from(first_day(a.i64_or(2, 1)?)?);
                let idx = ((n - 1 + fd - 1) % 7) as usize;
                let name = format::DAY_NAMES[idx];
                Value::String(if abbrev {
                    name[..3].to_string()
                } else {
                    name.to_string()
                })
            }
            "monthname" => {
                let n = a.i64(0)?;
                if !(1..=12).contains(&n) {
                    return Err(invalid());
                }
                let abbrev = match a.get(1) {
                    Some(v) => convert::to_bool(v)?,
                    None => false,
                };
                let name = format::MONTH_NAMES[n as usize - 1];
                Value::String(if abbrev {
                    name[..3].to_string()
                } else {
                    name.to_string()
                })
            }

            // ---- Math ----
            "abs" => {
                let v = a.req(0)?;
                if convert::to_f64(v)? < 0.0 {
                    convert::negate(v)?
                } else {
                    match v {
                        Value::Empty => Value::Integer(0),
                        Value::String(_) | Value::Boolean(_) => {
                            Value::Double(convert::to_f64(v)?.abs())
                        }
                        other => other.clone(),
                    }
                }
            }
            "sgn" => {
                let x = a.f64(0)?;
                Value::Integer(if x > 0.0 {
                    1
                } else if x < 0.0 {
                    -1
                } else {
                    0
                })
            }
            "sqr" => {
                let x = a.f64(0)?;
                if x < 0.0 {
                    return Err(invalid());
                }
                Value::Double(x.sqrt())
            }
            "exp" => {
                let r = a.f64(0)?.exp();
                if !r.is_finite() {
                    return Err(ScriptError::runtime(6));
                }
                Value::Double(r)
            }
            "log" => {
                let x = a.f64(0)?;
                if x <= 0.0 {
                    return Err(invalid());
                }
                Value::Double(x.ln())
            }
            "sin" => Value::Double(a.f64(0)?.sin()),
            "cos" => Value::Double(a.f64(0)?.cos()),
            "tan" => Value::Double(a.f64(0)?.tan()),
            "atn" => Value::Double(a.f64(0)?.atan()),
            "rnd" => {
                let n = match a.get(0) {
                    Some(v) => Some(convert::to_f64(v)?),
                    None => None,
                };
                Value::Single(self.rnd(n))
            }
            "randomize" => {
                let n = match a.get(0) {
                    Some(v) => convert::to_f64(v)?,
                    None => self.timer(),
                };
                self.randomize(n);
                Value::Empty
            }

            // ---- Interaction and the rest ----
            "array" => {
                let base = f.module.base;
                Value::array(Array::from_vec(
                    base,
                    a.0.into_iter()
                        .map(|v| if v.is_missing() { Value::Empty } else { v })
                        .collect(),
                ))
            }
            "iif" => {
                let c = self.plain(a.req(0)?.clone());
                let truth = !c.is_null() && convert::to_bool(&c)?;
                a.0.get(if truth { 1 } else { 2 })
                    .cloned()
                    .unwrap_or(Value::Empty)
            }
            "choose" => {
                let c = self.plain(a.req(0)?.clone());
                let i = convert::to_f64(&c)?.trunc();
                if i >= 1.0 && (i as usize) < a.0.len() {
                    a.0[i as usize].clone()
                } else {
                    Value::Null
                }
            }
            "switch" => {
                if !a.0.len().is_multiple_of(2) {
                    return Err(invalid());
                }
                let mut out = Value::Null;
                for pair in a.0.chunks(2) {
                    let c = self.plain(pair[0].clone());
                    if !c.is_null() && convert::to_bool(&c)? {
                        out = pair[1].clone();
                        break;
                    }
                }
                out
            }
            "msgbox" => {
                let prompt = convert::display(a.req(0)?);
                let buttons = a.i64_or(1, 0)? as i32;
                let title = a.str_or(2, "")?;
                Value::Integer(self.host.msg_box(&prompt, buttons, &title)? as i16)
            }
            "inputbox" => {
                let prompt = convert::display(a.req(0)?);
                let title = a.str_or(1, "")?;
                let default = a.str_or(2, "")?;
                Value::String(
                    self.host
                        .input_box(&prompt, &title, &default)?
                        .unwrap_or_default(),
                )
            }
            "doevents" => Value::Integer(0),
            "beep" => Value::Empty,
            "rgb" => {
                let c = |i: usize| -> R<i64> { Ok(a.i64(i)?.clamp(0, 255)) };
                Value::Long((c(0)? + c(1)? * 256 + c(2)? * 65_536) as i32)
            }
            "err" => Value::object(self.st.err.clone()),
            "erl" => Value::Long(f.erl as i32),
            "error" => {
                let n = match a.get(0) {
                    Some(v) => convert::to_i64(v)? as i32,
                    None => self.st.err.number(),
                };
                if n == 0 {
                    Value::String(String::new())
                } else if a.get(0).is_none() {
                    Value::String(self.st.err.state.borrow().description.clone())
                } else {
                    Value::String(standard_message(n).to_string())
                }
            }
            "createobject" => self.create_object(&a.str(0)?)?,
            "callbyname" => {
                let Value::Object(Some(o)) = a.req(0)?.clone() else {
                    return Err(ScriptError::runtime(424));
                };
                let name = a.str(1)?;
                let kind = a.i64(2)?;
                let rest: Vec<Arg> = a.0.iter().skip(3).map(|v| Arg::new(v.clone())).collect();
                self.call_by_name(&o, &name, kind, rest)?
            }
            _ => {
                return Err(ScriptError::new(
                    35,
                    format!("Sub or Function not defined: {key}"),
                ));
            }
        })
    }

    /// The value of an object's default member, or the value itself (lenient).
    fn plain(&mut self, v: Value) -> Value {
        match v {
            Value::Object(Some(_)) => self.deref(v.clone()).unwrap_or(v),
            other => other,
        }
    }
}

fn title_case(key: &str) -> String {
    let mut c = key.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}
