//! Excel's header and footer codes (ECMA-376 18.3.1.46): `&L`, `&C`, `&R`
//! start the left, centre and right sections; `&P` page number (`&P+n`,
//! `&P-n`), `&N` number of pages, `&D` date, `&T` time, `&F` file name,
//! `&Z` folder, `&A` sheet name, `&&` an ampersand; `&"Font,Style"` font
//! and style, `&nn` size in points, `&B` `&I` `&U` `&E` `&S` toggle bold,
//! italic, underline, double underline and strikethrough, `&X` / `&Y`
//! superscript / subscript, `&KRRGGBB` colour. `&G` (a picture) and theme
//! colours (`&KTTSNNN`) are skipped.

/// Values of the fields on one page.
#[derive(Debug, Clone, Default)]
pub struct Fields {
    pub page: i64,
    pub pages: i64,
    pub date: String,
    pub time: String,
    pub file: String,
    pub folder: String,
    pub sheet: String,
}

/// Formatting of a run of header text.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Font {
    /// Family; `None` for the workbook's default font.
    pub family: Option<String>,
    /// Points; `None` for the default (11).
    pub size: Option<f32>,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
    pub color: Option<u32>,
    /// Superscript (+1) or subscript (-1).
    pub shift: i8,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Run {
    pub text: String,
    pub font: Font,
}

/// One section: lines of runs.
pub type Section = Vec<Vec<Run>>;

/// The left, centre and right sections of a header or footer.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Parts {
    pub left: Section,
    pub center: Section,
    pub right: Section,
}

impl Parts {
    pub fn is_empty(&self) -> bool {
        [&self.left, &self.center, &self.right]
            .iter()
            .all(|s| s.iter().all(|l| l.iter().all(|r| r.text.is_empty())))
    }
}

fn push(section: &mut Section, text: &str, font: &Font) {
    if section.is_empty() {
        section.push(Vec::new());
    }
    for (i, piece) in text.split('\n').enumerate() {
        if i > 0 {
            section.push(Vec::new());
        }
        if piece.is_empty() {
            continue;
        }
        let line = section.last_mut().expect("a line");
        match line.last_mut() {
            Some(r) if r.font == *font => r.text.push_str(piece),
            _ => line.push(Run {
                text: piece.to_owned(),
                font: font.clone(),
            }),
        }
    }
}

/// Parse a header or footer string with the page's field values.
pub fn parse(s: &str, f: &Fields) -> Parts {
    let mut parts = Parts::default();
    // Text before any section code goes to the centre (Excel's reading).
    let mut which = 1;
    let mut font = Font::default();
    let chars: Vec<char> = s.replace("\r\n", "\n").chars().collect();
    let mut i = 0;
    let mut text = String::new();
    macro_rules! flush {
        () => {
            if !text.is_empty() {
                let sec = match which {
                    0 => &mut parts.left,
                    1 => &mut parts.center,
                    _ => &mut parts.right,
                };
                push(sec, &text, &font);
                text.clear();
            }
        };
    }
    while i < chars.len() {
        let c = chars[i];
        if c != '&' || i + 1 >= chars.len() {
            text.push(c);
            i += 1;
            continue;
        }
        let code = chars[i + 1];
        i += 2;
        match code {
            '&' => text.push('&'),
            'L' | 'C' | 'R' => {
                flush!();
                which = match code {
                    'L' => 0,
                    'C' => 1,
                    _ => 2,
                };
                font = Font::default();
            }
            'P' => {
                // &P+n / &P-n: the page number with an offset.
                let mut n = f.page;
                if i < chars.len() && (chars[i] == '+' || chars[i] == '-') {
                    let neg = chars[i] == '-';
                    let mut j = i + 1;
                    let mut v: i64 = 0;
                    while j < chars.len() && chars[j].is_ascii_digit() && j - i < 6 {
                        v = v * 10 + i64::from(chars[j].to_digit(10).unwrap_or(0));
                        j += 1;
                    }
                    if j > i + 1 {
                        n += if neg { -v } else { v };
                        i = j;
                    }
                }
                text.push_str(&n.to_string());
            }
            'N' => text.push_str(&f.pages.to_string()),
            'D' => text.push_str(&f.date),
            'T' => text.push_str(&f.time),
            'F' => text.push_str(&f.file),
            'Z' => text.push_str(&f.folder),
            'A' => text.push_str(&f.sheet),
            'G' => {}
            'B' | 'I' | 'U' | 'E' | 'S' | 'X' | 'Y' => {
                flush!();
                match code {
                    'B' => font.bold = !font.bold,
                    'I' => font.italic = !font.italic,
                    'U' | 'E' => font.underline = !font.underline,
                    'S' => font.strike = !font.strike,
                    'X' => font.shift = if font.shift == 1 { 0 } else { 1 },
                    _ => font.shift = if font.shift == -1 { 0 } else { -1 },
                }
            }
            '"' => {
                // &"Font Name,Style"
                flush!();
                let mut j = i;
                while j < chars.len() && chars[j] != '"' {
                    j += 1;
                }
                let spec: String = chars[i..j].iter().collect();
                i = (j + 1).min(chars.len());
                let (name, style) = spec.split_once(',').unwrap_or((&spec, ""));
                let name = name.trim();
                font.family = (!name.is_empty() && name != "-").then(|| name.to_owned());
                let style = style.to_ascii_lowercase();
                if !style.is_empty() {
                    font.bold = style.contains("bold");
                    font.italic = style.contains("italic") || style.contains("oblique");
                }
            }
            'K' => {
                flush!();
                let hex: String = chars[i..chars.len().min(i + 6)].iter().collect();
                if hex.len() == 6 {
                    let theme = matches!(hex.as_bytes()[2], b'S' | b's' | b'+' | b'-');
                    if theme {
                        // A theme colour ("01S050"): shown in the default
                        // colour.
                        font.color = None;
                        i += 6;
                    } else if let Ok(c) = u32::from_str_radix(&hex, 16) {
                        font.color = Some(c);
                        i += 6;
                    }
                }
            }
            d if d.is_ascii_digit() => {
                flush!();
                let mut j = i - 1;
                let mut v: u32 = 0;
                while j < chars.len() && chars[j].is_ascii_digit() && j < i + 2 {
                    v = v * 10 + chars[j].to_digit(10).unwrap_or(0);
                    j += 1;
                }
                i = j;
                if (1..=409).contains(&v) {
                    font.size = Some(v as f32);
                }
            }
            other => {
                // Unknown code: Excel shows nothing for it.
                let _ = other;
            }
        }
    }
    flush!();
    parts
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fields() -> Fields {
        Fields {
            page: 3,
            pages: 12,
            date: "11/11/2026".into(),
            time: "9:05 AM".into(),
            file: "book.xlsx".into(),
            folder: "/home/x/".into(),
            sheet: "Sales".into(),
        }
    }

    fn plain(s: &Section) -> String {
        s.iter()
            .map(|l| l.iter().map(|r| r.text.as_str()).collect::<String>())
            .collect::<Vec<_>>()
            .join("|")
    }

    #[test]
    fn sections_and_fields() {
        let p = parse("&LLeft &A&CPage &P of &N&R&D &T", &fields());
        assert_eq!(plain(&p.left), "Left Sales");
        assert_eq!(plain(&p.center), "Page 3 of 12");
        assert_eq!(plain(&p.right), "11/11/2026 9:05 AM");
        let p = parse("Page &P+10 &P-1 &F &Z&& more", &fields());
        assert_eq!(plain(&p.center), "Page 13 2 book.xlsx /home/x/& more");
        assert!(p.left.is_empty() && p.right.is_empty());
    }

    #[test]
    fn fonts() {
        let p = parse(
            "&L&\"Arial,Bold Italic\"&14Title&B off&C&KFF0000Red&K01S050 theme\nline 2",
            &fields(),
        );
        let l = &p.left[0];
        assert_eq!(l[0].text, "Title");
        assert_eq!(l[0].font.family.as_deref(), Some("Arial"));
        assert_eq!(l[0].font.size, Some(14.0));
        assert!(l[0].font.bold && l[0].font.italic);
        assert_eq!(l[1].text, " off");
        assert!(!l[1].font.bold && l[1].font.italic);
        let c = &p.center;
        assert_eq!(c[0][0].font.color, Some(0xFF0000));
        assert_eq!(c[0][1].text, " theme");
        assert_eq!(c[0][1].font.color, None);
        assert_eq!(plain(c), "Red theme|line 2");
        // Toggles: underline and strike.
        let p = parse("&U&Su&S&Uplain", &fields());
        assert!(p.center[0][0].font.underline && p.center[0][0].font.strike);
        assert!(!p.center[0][1].font.underline);
        assert!(parse("&L&C&R", &fields()).is_empty());
    }
}
