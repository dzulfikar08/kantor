//! Page setup: paper size, orientation and margins (the body's final
//! `w:sectPr`), and the save-time rewrite that touches only `w:pgSz` and
//! `w:pgMar` attributes inside the preserved section properties.
//!
//! Defaults: a new document is A4 portrait with Word's "Normal" margins
//! (2.54 cm on every side). Word's own default paper follows the Windows
//! region: US-English installs start on Letter, Indonesian/metric ones on
//! A4, and margins are 2.54 cm either way (checked 2026-11-07). Indonesian
//! offices also use F4 ("Folio", 215 × 330 mm) a lot, so it's a preset.

use crate::document::PageGeometry;

/// Twips per millimetre.
const TW_PER_MM: f64 = 1440.0 / 25.4;

/// Millimetres to twips (rounded).
pub fn mm_to_twips(mm: f64) -> u32 {
    (mm * TW_PER_MM).round().max(0.0) as u32
}

/// Twips to centimetres.
pub fn twips_to_cm(tw: u32) -> f64 {
    tw as f64 / TW_PER_MM / 10.0
}

/// Centimetres to twips (rounded, never negative).
pub fn cm_to_twips(cm: f64) -> u32 {
    mm_to_twips(cm * 10.0)
}

/// Paper sizes offered in the Page menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PaperSize {
    A4,
    Letter,
    Legal,
    A5,
    /// F4 / Folio, 215 × 330 mm (common in Indonesia).
    F4,
}

impl PaperSize {
    pub const ALL: [Self; 5] = [Self::A4, Self::Letter, Self::Legal, Self::A5, Self::F4];

    /// Portrait width and height in twips (Word's values).
    pub fn twips(self) -> (u32, u32) {
        match self {
            Self::A4 => (11906, 16838),
            Self::Letter => (12240, 15840),
            Self::Legal => (12240, 20160),
            Self::A5 => (8391, 11906),
            Self::F4 => (12189, 18709),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::A4 => "A4",
            Self::Letter => "Letter",
            Self::Legal => "Legal",
            Self::A5 => "A5",
            Self::F4 => "F4 / Folio",
        }
    }

    /// Size in words, e.g. `21 × 29.7 cm`.
    pub fn detail(self) -> &'static str {
        match self {
            Self::A4 => "21 × 29.7 cm",
            Self::Letter => "21.59 × 27.94 cm",
            Self::Legal => "21.59 × 35.56 cm",
            Self::A5 => "14.8 × 21 cm",
            Self::F4 => "21.5 × 33 cm",
        }
    }

    /// The preset matching a page (either orientation). Within about 1 mm,
    /// so Word's 8.5 × 13 in "Folio" also reads as F4.
    pub fn of(page: &PageGeometry) -> Option<Self> {
        let (a, b) = (page.width.min(page.height), page.width.max(page.height));
        Self::ALL.into_iter().find(|p| {
            let (w, h) = p.twips();
            a.abs_diff(w) <= 60 && b.abs_diff(h) <= 60
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Orientation {
    Portrait,
    Landscape,
}

/// Word's margin presets.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MarginPreset {
    /// 2.54 cm all round.
    Normal,
    /// 1.27 cm all round.
    Narrow,
    /// 2.54 cm top/bottom, 1.91 cm left/right.
    Moderate,
    /// 2.54 cm top/bottom, 5.08 cm left/right.
    Wide,
}

impl MarginPreset {
    pub const ALL: [Self; 4] = [Self::Normal, Self::Narrow, Self::Moderate, Self::Wide];

    /// `(top, bottom, left, right)` in twips.
    pub fn twips(self) -> (u32, u32, u32, u32) {
        match self {
            Self::Normal => (1440, 1440, 1440, 1440),
            Self::Narrow => (720, 720, 720, 720),
            Self::Moderate => (1440, 1440, 1080, 1080),
            Self::Wide => (1440, 1440, 2880, 2880),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Normal => "Normal",
            Self::Narrow => "Narrow",
            Self::Moderate => "Moderate",
            Self::Wide => "Wide",
        }
    }

    pub fn detail(self) -> &'static str {
        match self {
            Self::Normal => "2.54 cm all sides",
            Self::Narrow => "1.27 cm all sides",
            Self::Moderate => "2.54 cm top/bottom, 1.91 cm sides",
            Self::Wide => "2.54 cm top/bottom, 5.08 cm sides",
        }
    }

    pub fn of(page: &PageGeometry) -> Option<Self> {
        let m = (
            page.margin_top,
            page.margin_bottom,
            page.margin_left,
            page.margin_right,
        );
        Self::ALL.into_iter().find(|p| p.twips() == m)
    }
}

impl PageGeometry {
    pub fn orientation(&self) -> Orientation {
        if self.width > self.height {
            Orientation::Landscape
        } else {
            Orientation::Portrait
        }
    }

    /// This page in another orientation: width and height swap, and so do
    /// the margins (top ↔ left, bottom ↔ right), as when Word turns a page.
    pub fn with_orientation(self, o: Orientation) -> Self {
        if self.orientation() == o {
            return self;
        }
        Self {
            width: self.height,
            height: self.width,
            margin_top: self.margin_left,
            margin_left: self.margin_top,
            margin_bottom: self.margin_right,
            margin_right: self.margin_bottom,
        }
    }

    /// This page on another paper, keeping orientation and margins.
    pub fn with_paper(self, p: PaperSize) -> Self {
        let (w, h) = p.twips();
        let (width, height) = match self.orientation() {
            Orientation::Portrait => (w, h),
            Orientation::Landscape => (h, w),
        };
        Self {
            width,
            height,
            ..self
        }
        .clamped()
    }

    pub fn with_margins(self, m: MarginPreset) -> Self {
        let (t, b, l, r) = m.twips();
        Self {
            margin_top: t,
            margin_bottom: b,
            margin_left: l,
            margin_right: r,
            ..self
        }
        .clamped()
    }

    /// Margins shrunk so at least 1 cm of text area remains each way.
    pub fn clamped(mut self) -> Self {
        const MIN: u32 = 567;
        let fit = |a: &mut u32, b: &mut u32, total: u32| {
            let room = total.saturating_sub(MIN);
            if *a + *b > room {
                let over = *a + *b - room;
                let cut_a = over.min(*a);
                *a -= cut_a;
                *b -= (over - cut_a).min(*b);
            }
        };
        fit(&mut self.margin_left, &mut self.margin_right, self.width);
        fit(&mut self.margin_top, &mut self.margin_bottom, self.height);
        self
    }
}

// ── save-time rewrite ───────────────────────────────────────────────────────

/// A child start tag of the sectPr: byte range and local name.
struct Tag {
    start: usize,
    end: usize,
    name: String,
}

/// Direct child start tags of the root element of `xml`.
fn child_tags(xml: &str) -> Vec<Tag> {
    let mut out = Vec::new();
    let mut depth = 0usize;
    let mut i = 0;
    let b = xml.as_bytes();
    while let Some(off) = xml[i..].find('<') {
        let s = i + off;
        // Find the tag end, skipping quoted attribute values.
        let mut j = s + 1;
        let mut quote: Option<u8> = None;
        while j < b.len() {
            match (quote, b[j]) {
                (Some(q), c) if c == q => quote = None,
                (None, b'"' | b'\'') => quote = Some(b[j]),
                (None, b'>') => break,
                _ => {}
            }
            j += 1;
        }
        let end = (j + 1).min(b.len());
        let tag = &xml[s..end];
        if tag.starts_with("<?") || tag.starts_with("<!") {
        } else if tag.starts_with("</") {
            depth = depth.saturating_sub(1);
        } else {
            let self_closing = tag.ends_with("/>");
            if depth == 1 {
                let name: String = tag[1..]
                    .chars()
                    .take_while(|c| !c.is_whitespace() && *c != '/' && *c != '>')
                    .collect();
                let local = name.rsplit(':').next().unwrap_or(&name).to_owned();
                out.push(Tag {
                    start: s,
                    end,
                    name: local,
                });
            }
            if !self_closing {
                depth += 1;
            }
        }
        i = end;
    }
    out
}

/// The `prefix:` of the root element (`w:` usually).
fn root_prefix(xml: &str) -> &str {
    let s = xml.find('<').map_or(0, |i| i + 1);
    let name_end = xml[s..]
        .find(|c: char| c.is_whitespace() || c == '>' || c == '/')
        .map_or(xml.len(), |e| s + e);
    match xml[s..name_end].find(':') {
        Some(c) => &xml[s..s + c + 1],
        None => "",
    }
}

/// Attributes of a start tag as `(qualified name, value range)`.
fn attrs(tag: &str) -> Vec<(String, std::ops::Range<usize>)> {
    let b = tag.as_bytes();
    let mut out = Vec::new();
    let mut i = tag.find(|c: char| c.is_whitespace()).unwrap_or(tag.len());
    while i < b.len() {
        while i < b.len() && (b[i] as char).is_whitespace() {
            i += 1;
        }
        let ns = i;
        while i < b.len()
            && b[i] != b'='
            && !(b[i] as char).is_whitespace()
            && b[i] != b'/'
            && b[i] != b'>'
        {
            i += 1;
        }
        if i == ns {
            break;
        }
        let name = tag[ns..i].to_owned();
        while i < b.len() && (b[i] == b'=' || (b[i] as char).is_whitespace()) {
            i += 1;
        }
        if i >= b.len() || !(b[i] == b'"' || b[i] == b'\'') {
            break;
        }
        let q = b[i];
        let vs = i + 1;
        let ve = tag[vs..].find(q as char).map_or(tag.len(), |e| vs + e);
        out.push((name, vs..ve));
        i = ve + 1;
    }
    out
}

fn local_name(q: &str) -> &str {
    q.rsplit(':').next().unwrap_or(q)
}

/// Set attributes on a start tag. `Some(v)` replaces the value only if it
/// differs per `same`, or appends `prefix:name="v"`; `None` removes it.
/// Everything else in the tag is kept byte for byte.
fn set_attrs(
    tag: &str,
    prefix: &str,
    changes: &[(&str, Option<String>)],
    same: impl Fn(&str, &str, &str) -> bool,
) -> String {
    let found = attrs(tag);
    // Edits as (range, replacement), applied back to front.
    let mut edits: Vec<(std::ops::Range<usize>, String)> = Vec::new();
    let mut append = String::new();
    for (name, val) in changes {
        let hit = found.iter().find(|(q, _)| local_name(q) == *name);
        match (hit, val) {
            (Some((_, r)), Some(v)) => {
                if !same(name, &tag[r.clone()], v) {
                    edits.push((r.clone(), v.clone()));
                }
            }
            (Some((q, r)), None) => {
                // Remove ` name="value"` with its leading whitespace.
                let name_start = r.start - 2 - q.len();
                let ws = tag[..name_start].trim_end().len();
                edits.push((ws..r.end + 1, String::new()));
            }
            (None, Some(v)) => append.push_str(&format!(" {prefix}{name}=\"{v}\"")),
            (None, None) => {}
        }
    }
    let mut out = tag.to_owned();
    if !append.is_empty() {
        let at = if out.ends_with("/>") {
            out.len() - 2
        } else {
            out.len() - 1
        };
        // Keep a space before "/>" where the original had one.
        let at = out[..at].trim_end().len();
        out.insert_str(at, &append);
    }
    edits.sort_by_key(|(r, _)| std::cmp::Reverse(r.start));
    for (r, v) in edits {
        out.replace_range(r, &v);
    }
    out
}

/// Twip values compare numerically, sign ignored (Word allows negative
/// top/bottom margins meaning "fixed": a value equal in size is unchanged).
fn same_twips(_: &str, old: &str, new: &str) -> bool {
    match (old.trim().parse::<i64>(), new.parse::<i64>()) {
        (Ok(a), Ok(b)) => a.unsigned_abs() == b.unsigned_abs(),
        _ => old == new,
    }
}

/// `sect_pr` (a whole `<w:sectPr>…</w:sectPr>`) with page size, orientation
/// and margins from `page`. Only the attributes whose values differ change;
/// a missing `pgSz`/`pgMar` is inserted where the schema wants it. Every
/// other byte is kept.
pub fn update_sect_pr(sect_pr: &str, page: &PageGeometry) -> String {
    let p = root_prefix(sect_pr).to_owned();
    let tags = child_tags(sect_pr);
    let landscape = page.orientation() == Orientation::Landscape;
    let mut out = sect_pr.to_owned();
    // Edits (range, replacement), applied back to front.
    let mut edits: Vec<(std::ops::Range<usize>, String)> = Vec::new();

    let size_changes = [
        ("w", Some(page.width.to_string())),
        ("h", Some(page.height.to_string())),
    ];
    let orient = if landscape {
        Some("landscape".to_owned())
    } else {
        None
    };
    let margin_changes = [
        ("top", Some(page.margin_top.to_string())),
        ("right", Some(page.margin_right.to_string())),
        ("bottom", Some(page.margin_bottom.to_string())),
        ("left", Some(page.margin_left.to_string())),
    ];
    let pg_sz = tags.iter().find(|t| t.name == "pgSz");
    let pg_mar = tags.iter().find(|t| t.name == "pgMar");

    match pg_sz {
        Some(t) => {
            let tag = &sect_pr[t.start..t.end];
            let mut new = set_attrs(tag, &p, &size_changes, same_twips);
            let has_orient = attrs(tag).iter().any(|(q, _)| local_name(q) == "orient");
            // Portrait: drop an "orient=landscape" (absent means portrait);
            // leave an explicit "portrait" alone.
            let o = match (&orient, has_orient) {
                (Some(_), _) => orient.clone(),
                (None, true) => {
                    let cur = attrs(tag)
                        .into_iter()
                        .find(|(q, _)| local_name(q) == "orient")
                        .map(|(_, r)| tag[r].to_owned());
                    (cur.as_deref() != Some("landscape")).then(|| cur.unwrap_or_default())
                }
                (None, false) => None,
            };
            if has_orient || o.is_some() {
                new = set_attrs(&new, &p, &[("orient", o)], |_, a, b| a == b);
            }
            if new != tag {
                edits.push((t.start..t.end, new));
            }
        }
        None => {
            let mut el = format!("<{p}pgSz {p}w=\"{}\" {p}h=\"{}\"", page.width, page.height);
            if landscape {
                el.push_str(&format!(" {p}orient=\"landscape\""));
            }
            el.push_str("/>");
            // After headerReference/footerReference/footnotePr/endnotePr/type.
            const BEFORE: [&str; 5] = [
                "headerReference",
                "footerReference",
                "footnotePr",
                "endnotePr",
                "type",
            ];
            let at = tags
                .iter()
                .find(|t| !BEFORE.contains(&t.name.as_str()))
                .map(|t| t.start)
                .unwrap_or_else(|| sect_pr.rfind("</").unwrap_or(sect_pr.len()));
            edits.push((at..at, el));
        }
    }
    match pg_mar {
        Some(t) => {
            let tag = &sect_pr[t.start..t.end];
            let new = set_attrs(tag, &p, &margin_changes, same_twips);
            if new != tag {
                edits.push((t.start..t.end, new));
            }
        }
        None => {
            let el = format!(
                "<{p}pgMar {p}top=\"{}\" {p}right=\"{}\" {p}bottom=\"{}\" {p}left=\"{}\" \
                 {p}header=\"708\" {p}footer=\"708\" {p}gutter=\"0\"/>",
                page.margin_top, page.margin_right, page.margin_bottom, page.margin_left
            );
            let at = match pg_sz {
                Some(t) => t.end,
                None => edits.last().map_or(sect_pr.len(), |(r, _)| r.start),
            };
            // Inserted after any new pgSz at the same point.
            edits.insert(0, (at..at, el));
        }
    }
    // Stable order: by start descending; equal starts keep pgMar after pgSz
    // because pgMar's edit was pushed first and replace_range at the same
    // point inserts before earlier text.
    edits.sort_by_key(|(r, _)| std::cmp::Reverse(r.start));
    for (r, v) in edits {
        out.replace_range(r, &v);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECT: &str = r#"<w:sectPr w:rsidR="00AB12CD"><w:headerReference w:type="default" r:id="rId8"/><w:pgSz w:w="12240" w:h="15840" w:code="1"/><w:pgMar w:top="-1440" w:right="1080" w:bottom="1440" w:left="1080" w:header="720" w:footer="720" w:gutter="0"/><w:cols w:space="720"/><w:docGrid w:linePitch="360"/></w:sectPr>"#;

    fn letter() -> PageGeometry {
        PageGeometry {
            width: 12240,
            height: 15840,
            margin_top: 1440,
            margin_bottom: 1440,
            margin_left: 1080,
            margin_right: 1080,
        }
    }

    #[test]
    fn unchanged_page_keeps_every_byte() {
        assert_eq!(update_sect_pr(SECT, &letter()), SECT);
    }

    #[test]
    fn only_changed_attributes_are_rewritten() {
        let page = letter()
            .with_paper(PaperSize::A4)
            .with_orientation(Orientation::Landscape);
        let out = update_sect_pr(SECT, &page);
        assert_eq!(
            out,
            r#"<w:sectPr w:rsidR="00AB12CD"><w:headerReference w:type="default" r:id="rId8"/><w:pgSz w:w="16838" w:h="11906" w:code="1" w:orient="landscape"/><w:pgMar w:top="1080" w:right="1440" w:bottom="1080" w:left="1440" w:header="720" w:footer="720" w:gutter="0"/><w:cols w:space="720"/><w:docGrid w:linePitch="360"/></w:sectPr>"#
        );
        // Back to portrait drops the orient attribute again.
        let back = update_sect_pr(&out, &page.with_orientation(Orientation::Portrait));
        assert!(
            back.contains(r#"<w:pgSz w:w="11906" w:h="16838" w:code="1"/>"#),
            "{back}"
        );
    }

    #[test]
    fn missing_elements_are_inserted_in_schema_order() {
        let s = r#"<w:sectPr><w:headerReference w:type="default" r:id="rId8"/><w:cols w:space="720"/></w:sectPr>"#;
        let out = update_sect_pr(s, &PageGeometry::default());
        assert_eq!(
            out,
            r#"<w:sectPr><w:headerReference w:type="default" r:id="rId8"/><w:pgSz w:w="11906" w:h="16838"/><w:pgMar w:top="1440" w:right="1440" w:bottom="1440" w:left="1440" w:header="708" w:footer="708" w:gutter="0"/><w:cols w:space="720"/></w:sectPr>"#
        );
        let empty = update_sect_pr("<w:sectPr></w:sectPr>", &PageGeometry::default());
        assert!(empty.starts_with("<w:sectPr><w:pgSz "), "{empty}");
        assert!(empty.ends_with("w:gutter=\"0\"/></w:sectPr>"), "{empty}");
    }

    #[test]
    fn paper_presets() {
        assert_eq!(PaperSize::of(&PageGeometry::default()), Some(PaperSize::A4));
        let f4 = PageGeometry::default().with_paper(PaperSize::F4);
        assert_eq!((f4.width, f4.height), (12189, 18709));
        assert!((twips_to_cm(f4.width) - 21.5).abs() < 0.01);
        assert!((twips_to_cm(f4.height) - 33.0).abs() < 0.01);
        // Word's 8.5 × 13 in Folio counts as F4.
        let folio = PageGeometry {
            width: 12240,
            height: 18720,
            ..f4
        };
        assert_eq!(PaperSize::of(&folio), Some(PaperSize::F4));
        let land = PageGeometry::default()
            .with_paper(PaperSize::Letter)
            .with_orientation(Orientation::Landscape);
        assert_eq!((land.width, land.height), (15840, 12240));
        assert_eq!(PaperSize::of(&land), Some(PaperSize::Letter));
        // Changing paper keeps the orientation.
        let legal = land.with_paper(PaperSize::Legal);
        assert_eq!((legal.width, legal.height), (20160, 12240));
        for m in MarginPreset::ALL {
            assert_eq!(MarginPreset::of(&legal.with_margins(m)), Some(m));
        }
        assert_eq!(cm_to_twips(2.54), 1440);
        assert_eq!(cm_to_twips(1.27), 720);
    }

    #[test]
    fn margins_never_eat_the_page() {
        let mut p = PageGeometry::default().with_paper(PaperSize::A5);
        p.margin_left = 6000;
        p.margin_right = 6000;
        let p = p.clamped();
        assert!(p.width - p.margin_left - p.margin_right >= 567);
    }
}
