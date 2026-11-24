//! A deck's look: built-in presets (`"preset": "consulting-navy"`), named
//! text styles (`"style": "eyebrow"` on elements, paragraphs, runs and
//! cells), the colour palette (`"color": "muted"`), slide furniture (the
//! header with eyebrow, title and subtitle, the footer with page numbers,
//! cover slides) and transitions. Everything ends up as ordinary elements
//! ([`super::slides_author`]), so a deck stays plain, editable shapes.

use std::collections::BTreeMap;
use std::sync::Arc;

use anyhow::{Context as _, Result, anyhow, bail};
use serde_json::{Map, Value, json};
use slides_engine::author::{ElementKind, TransitionSpec};
use slides_engine::{Emu, Presentation, Rgb, SlideSize};

use slides_engine::author::Element;

use super::slides_author::{Ctx, element, length, object, opt_bool, opt_str};

const IN: Emu = 914_400;
const PT: Emu = 12_700;

fn e(v: Emu) -> Value {
    json!(format!("{v}emu"))
}

// ── presets ────────────────────────────────────────────────────────────────

/// The built-in looks.
pub const PRESETS: [&str; 4] = ["consulting-navy", "warm", "mono", "teal"];

/// Text styles every deck has (presets and decks change or add to them).
/// Colours are palette names, `heading` / `body` the theme's fonts.
fn base_styles() -> Value {
    json!({
        "eyebrow": {"size": 11, "bold": true, "caps": true, "letter_spacing": 1, "color": "primary"},
        "title": {"font": "heading", "size": 30, "bold": true, "color": "ink", "line_spacing": 1.0},
        "subtitle": {"size": 15, "color": "muted", "line_spacing": 1.1},
        "body": {"size": 16, "color": "ink", "line_spacing": 1.15},
        "small": {"size": 12, "color": "muted", "line_spacing": 1.2},
        "note": {"size": 9, "color": "muted"},
        "label": {"size": 10.5, "bold": true, "caps": true, "letter_spacing": 0.5, "color": "muted"},
        "card_title": {"size": 16, "bold": true, "color": "ink", "line_spacing": 1.05},
        "stat": {"font": "heading", "size": 40, "bold": true, "color": "ink", "line_spacing": 1.0},
        "number": {"font": "heading", "size": 24, "bold": true, "color": "primary"},
        "badge": {"size": 12, "bold": true, "color": "primary"},
        "table": {"size": 12, "color": "ink"},
        "callout": {"size": 13, "color": "ink", "line_spacing": 1.15},
        "table_header": {"size": 10, "bold": true, "caps": true, "letter_spacing": 0.5, "color": "white"},
        "footer": {"size": 10, "color": "muted"},
        "cover_eyebrow": {"size": 12, "bold": true, "caps": true, "letter_spacing": 1, "color": "accent_light"},
        "cover_title": {"font": "heading", "size": 44, "bold": true, "color": "white", "line_spacing": 1.0},
        "cover_subtitle": {"size": 17, "color": "on_dark_muted", "line_spacing": 1.2},
    })
}

/// Header, footer and cover shared by the presets.
fn furniture() -> Value {
    json!({
        "header": {},
        "footer": {"page_numbers": true, "bar": {"color": "primary", "height": "0.08in"}},
        "cover": {},
    })
}

/// A preset: theme, palette, styles, header, footer, cover.
pub fn preset(name: &str) -> Result<Value> {
    let mut v = match name {
        "consulting-navy" | "navy" => json!({
            "theme": {"name": "Consulting Navy", "fonts": {"heading": "Calibri", "body": "Calibri"},
                "colors": {"dk1": "#0B1726", "lt1": "#FFFFFF", "dk2": "#061E3F", "lt2": "#F6F8FB",
                    "accent1": "#004FA7", "accent2": "#F29D1E", "accent3": "#2E8B57", "accent4": "#4C99F8",
                    "accent5": "#5B6676", "accent6": "#C2412D", "hlink": "#004FA7", "folHlink": "#5B6676"}},
            "palette": {"muted": "#5B6676", "divider": "#DDE3EA", "tint": "#E8F1FC", "paper": "#F6F8FB",
                "accent_light": "#4C99F8", "on_dark_muted": "#C9D6E8", "success": "#2E8B57", "danger": "#C2412D"},
        }),
        "warm" => json!({
            "theme": {"name": "Warm", "fonts": {"heading": "Georgia", "body": "Calibri"},
                "colors": {"dk1": "#2B2118", "lt1": "#FFFFFF", "dk2": "#3D2B1F", "lt2": "#FBF6F0",
                    "accent1": "#B4532A", "accent2": "#E0A43A", "accent3": "#5E7D4F", "accent4": "#2F6F73",
                    "accent5": "#8C6E5A", "accent6": "#A23B3B", "hlink": "#B4532A", "folHlink": "#8C6E5A"}},
            "palette": {"muted": "#7A6A5C", "divider": "#E8DDD2", "tint": "#F6E7DE", "paper": "#FBF6F0",
                "accent_light": "#E8A27F", "on_dark_muted": "#E3D3C4", "success": "#5E7D4F", "danger": "#A23B3B"},
            "styles": {"title": {"bold": false, "size": 32}, "stat": {"bold": false}, "cover_title": {"bold": false, "size": 46}},
        }),
        "mono" => json!({
            "theme": {"name": "Mono", "fonts": {"heading": "Arial", "body": "Arial"},
                "colors": {"dk1": "#111111", "lt1": "#FFFFFF", "dk2": "#1A1A1A", "lt2": "#F4F4F4",
                    "accent1": "#111111", "accent2": "#E63946", "accent3": "#2A9D8F", "accent4": "#6C757D",
                    "accent5": "#ADB5BD", "accent6": "#E63946", "hlink": "#111111", "folHlink": "#6C757D"}},
            "palette": {"muted": "#6B6B6B", "divider": "#DDDDDD", "tint": "#EFEFEF", "paper": "#F4F4F4",
                "accent_light": "#E63946", "on_dark_muted": "#C8C8C8", "success": "#2A9D8F", "danger": "#E63946"},
            "styles": {"eyebrow": {"color": "highlight"}, "number": {"color": "ink"}, "badge": {"color": "ink"}},
        }),
        "teal" => json!({
            "theme": {"name": "Teal", "fonts": {"heading": "Calibri", "body": "Calibri"},
                "colors": {"dk1": "#0F172A", "lt1": "#FFFFFF", "dk2": "#042F2E", "lt2": "#F1F5F4",
                    "accent1": "#0F766E", "accent2": "#F59E0B", "accent3": "#16A34A", "accent4": "#0EA5E9",
                    "accent5": "#64748B", "accent6": "#DC2626", "hlink": "#0F766E", "folHlink": "#64748B"}},
            "palette": {"muted": "#64748B", "divider": "#E2E8F0", "tint": "#E0F2F1", "paper": "#F1F5F4",
                "accent_light": "#5EEAD4", "on_dark_muted": "#CCE3E1", "success": "#16A34A", "danger": "#DC2626"},
        }),
        other => bail!("`preset` \"{other}\" is not one of {}", PRESETS.join(", ")),
    };
    merge(&mut v, &furniture());
    Ok(v)
}

/// `over` merged into `base` (objects key by key, anything else replaced).
pub fn merge(base: &mut Value, over: &Value) {
    match (base, over) {
        (Value::Object(b), Value::Object(o)) => {
            for (k, v) in o {
                match b.get_mut(k) {
                    Some(bv) if bv.is_object() && v.is_object() => merge(bv, v),
                    _ => {
                        b.insert(k.clone(), v.clone());
                    }
                }
            }
        }
        (b, o) => *b = o.clone(),
    }
}

/// The deck's `theme`: its preset's with the deck's own settings over it.
pub fn deck_theme(deck: &Map<String, Value>) -> Result<Option<Value>> {
    let mut theme = match deck.get("preset").and_then(Value::as_str) {
        Some(name) => preset(name)?["theme"].clone(),
        None => Value::Null,
    };
    if let Some(t) = deck.get("theme").filter(|t| !t.is_null()) {
        if theme.is_null() {
            theme = t.clone();
        } else {
            merge(&mut theme, t);
        }
    }
    Ok((!theme.is_null()).then_some(theme))
}

// ── the look ───────────────────────────────────────────────────────────────

/// What a deck's elements draw on: styles, colours, furniture.
#[derive(Clone, Debug)]
pub struct Look {
    /// Text styles by name (objects of text keys).
    pub styles: Map<String, Value>,
    /// Colours by name: the colour and the theme slot it follows, if any.
    pub palette: BTreeMap<String, (Rgb, Option<String>)>,
    pub header: Option<Map<String, Value>>,
    pub footer: Option<Map<String, Value>>,
    pub cover: Map<String, Value>,
    /// The content area under the header: x, y, w, h.
    pub content: [Emu; 4],
    /// Space between cards, columns and rows.
    pub gap: Emu,
    pub transition: Option<TransitionSpec>,
    pub size: SlideSize,
}

/// Palette names (besides theme colour names, which always work).
pub const PALETTE: [&str; 17] = [
    "primary",
    "highlight",
    "ink",
    "muted",
    "divider",
    "paper",
    "tint",
    "dark",
    "white",
    "accent_light",
    "on_dark_muted",
    "info",
    "success",
    "warning",
    "danger",
    "black",
    "on_primary",
];

fn mix(a: Rgb, b: Rgb, t: f64) -> Rgb {
    let m = |x: u8, y: u8| {
        (x as f64 + (y as f64 - x as f64) * t)
            .round()
            .clamp(0.0, 255.0) as u8
    };
    Rgb([m(a.0[0], b.0[0]), m(a.0[1], b.0[1]), m(a.0[2], b.0[2])])
}

const WHITE: Rgb = Rgb([255, 255, 255]);
const BLACK: Rgb = Rgb([0, 0, 0]);

impl Look {
    /// The default look of a presentation (no header or footer): styles and
    /// a palette from its theme.
    pub fn plain(p: &Presentation) -> Look {
        Self::build(p, &Map::new(), &Ctx::default()).unwrap_or_else(|_| Self::fallback(p.size))
    }

    /// Without a presentation: Office colours.
    fn fallback(size: SlideSize) -> Look {
        static OFFICE: std::sync::OnceLock<Look> = std::sync::OnceLock::new();
        let mut l = OFFICE
            .get_or_init(|| {
                Self::build(&Presentation::new(), &Map::new(), &Ctx::default())
                    .expect("the default look")
            })
            .clone();
        l.size = size;
        l.content = default_content(size);
        l
    }

    /// From a deck's `preset`, `styles`, `palette`, `header`, `footer`,
    /// `cover`, `content` and `transition`, over the presentation's theme.
    pub fn build(p: &Presentation, deck: &Map<String, Value>, cx: &Ctx) -> Result<Look> {
        let preset = match opt_str(deck, "preset")? {
            Some(n) => preset(n)?,
            None => json!({}),
        };
        let slot = |s: &str| p.theme_color(s);
        let theme_rgb = |s: &str, d: Rgb| slot(s).unwrap_or(d);
        let (ink, white) = (theme_rgb("dk1", BLACK), theme_rgb("lt1", WHITE));
        let accent1 = theme_rgb("accent1", Rgb([0x44, 0x72, 0xC4]));
        let accent2 = theme_rgb("accent2", Rgb([0xED, 0x7D, 0x31]));
        let dark = theme_rgb("dk2", mix(ink, accent1, 0.35));
        let paper = theme_rgb("lt2", Rgb([0xF4, 0xF4, 0xF4]));
        let mut palette: BTreeMap<String, (Rgb, Option<String>)> = BTreeMap::new();
        let mut put = |k: &str, c: Rgb, s: Option<&str>| {
            palette.insert(k.to_owned(), (c, s.map(str::to_owned)));
        };
        put("primary", accent1, Some("accent1"));
        put("info", accent1, Some("accent1"));
        put("highlight", accent2, Some("accent2"));
        put("warning", accent2, Some("accent2"));
        put("ink", ink, Some("tx1"));
        put("dark", dark, Some("tx2"));
        put("paper", paper, Some("bg2"));
        put("white", white, Some("bg1"));
        put("on_primary", white, Some("bg1"));
        put("black", BLACK, None);
        put("muted", mix(ink, white, 0.4), None);
        put("divider", mix(ink, white, 0.86), None);
        put("tint", mix(accent1, white, 0.9), None);
        put("accent_light", mix(accent1, white, 0.45), None);
        put("on_dark_muted", mix(white, dark, 0.22), None);
        put("success", Rgb([0x2E, 0x8B, 0x57]), None);
        put("danger", Rgb([0xC2, 0x41, 0x2D]), None);
        // Preset, then deck palette.
        for src in [preset.get("palette"), deck.get("palette")]
            .into_iter()
            .flatten()
            .filter(|v| !v.is_null())
        {
            let o = object(src, "`palette`", &PALETTE)?;
            for (k, v) in o {
                let s = v
                    .as_str()
                    .with_context(|| format!("palette `{k}` must be a colour string"))?;
                let c = match super::slides_author::color(v, k)?.base {
                    slides_engine::author::ColorBase::Rgb(c) => (c, None),
                    slides_engine::author::ColorBase::Scheme(name) => {
                        let slot_name = match name.as_str() {
                            "tx1" => "dk1",
                            "bg1" => "lt1",
                            "tx2" => "dk2",
                            "bg2" => "lt2",
                            n => n,
                        };
                        let c = slot(slot_name)
                            .ok_or_else(|| anyhow!("palette `{k}`: the theme has no {s}"))?;
                        (c, Some(name))
                    }
                };
                palette.insert(k.clone(), c);
            }
        }
        // Styles: base, preset, deck.
        let mut styles = base_styles();
        if let Some(s) = preset.get("styles") {
            merge(&mut styles, s);
        }
        if let Some(s) = deck.get("styles").filter(|v| !v.is_null()) {
            let so = s
                .as_object()
                .context("`styles` must be an object of named styles")?;
            for (name, st) in so {
                if name.is_empty() || name.len() > 64 {
                    bail!("style names are 1 to 64 characters");
                }
                object(st, &format!("style \"{name}\""), STYLE_DEF_KEYS)?;
            }
            merge(&mut styles, s);
        }
        let styles = match styles {
            Value::Object(m) => m,
            _ => Map::new(),
        };
        // Header, footer, cover: the preset's, then the deck's (false: none).
        let part = |key: &str| -> Result<Option<Map<String, Value>>> {
            let mut v = preset.get(key).cloned().unwrap_or(Value::Null);
            match deck.get(key) {
                Some(Value::Bool(false)) => return Ok(None),
                Some(Value::Bool(true)) if v.is_null() => v = json!({}),
                Some(Value::Bool(true)) | None | Some(Value::Null) => {}
                Some(o @ Value::Object(_)) => {
                    if v.is_null() {
                        v = json!({});
                    }
                    merge(&mut v, o);
                }
                Some(_) => bail!("`{key}` must be an object or false"),
            }
            Ok(v.as_object().cloned())
        };
        let header = part("header")?;
        let footer = part("footer")?;
        let cover = part("cover")?.unwrap_or_default();
        if let Some(h) = &header {
            object(&Value::Object(h.clone()), "`header`", HEADER_KEYS)?;
        }
        if let Some(f) = &footer {
            object(&Value::Object(f.clone()), "`footer`", FOOTER_KEYS)?;
        }
        object(&Value::Object(cover.clone()), "`cover`", COVER_KEYS)?;
        let size = cx.size.unwrap_or(p.size);
        let mut content = default_content(size);
        let mut gap = (0.24 * IN as f64) as Emu;
        if let Some(c) = deck.get("content").filter(|v| !v.is_null()) {
            let o = object(c, "`content`", &["x", "y", "w", "h", "gap"])?;
            for (i, k) in ["x", "y", "w", "h"].iter().enumerate() {
                if let Some(v) = o.get(*k).filter(|v| !v.is_null()) {
                    content[i] = length(v, cx, k)?;
                }
            }
            if let Some(g) = o.get("gap").filter(|v| !v.is_null()) {
                gap = length(g, cx, "gap")?.max(0);
            }
        }
        let transition = match deck.get("transition") {
            None | Some(Value::Null) => None,
            Some(t) => transition(t)?,
        };
        Ok(Look {
            styles,
            palette,
            header,
            footer,
            cover,
            content,
            gap,
            transition,
            size,
        })
    }

    /// A palette colour as a colour value for elements: the theme slot it
    /// follows, else `#RRGGBB`.
    pub fn color(&self, name: &str) -> Option<Value> {
        let (c, slot) = self.palette.get(name)?;
        Some(match slot {
            Some(s) => json!(s),
            None => json!(format!("#{}", c.hex())),
        })
    }

    /// A palette colour (or `#RRGGBB`) as RGB.
    pub fn rgb(&self, name: &str) -> Rgb {
        if let Some((c, _)) = self.palette.get(name) {
            return *c;
        }
        name.strip_prefix('#')
            .and_then(Rgb::parse)
            .unwrap_or(Rgb([0x80, 0x80, 0x80]))
    }

    /// `#RRGGBB` of `a` mixed with `b` (palette names) by `t`.
    pub fn mixed(&self, a: &str, b: &str, t: f64) -> Value {
        json!(format!("#{}", mix(self.rgb(a), self.rgb(b), t).hex()))
    }

    /// A style's font size in points (default `d`).
    pub fn size_of(&self, style: &str, d: f64) -> f64 {
        self.styles
            .get(style)
            .and_then(|s| s.get("size"))
            .and_then(Value::as_f64)
            .unwrap_or(d)
    }

    /// The height of one line of `style` (single spacing ≈ 1.2 × the size).
    pub fn line_h(&self, style: &str, d: f64) -> Emu {
        let ls = self
            .styles
            .get(style)
            .and_then(|s| s.get("line_spacing"))
            .and_then(Value::as_f64)
            .unwrap_or(1.0);
        (self.size_of(style, d) * 1.2 * ls * PT as f64).round() as Emu
    }

    /// About how many lines `text` takes in `style` at width `w`.
    pub fn lines(&self, style: &str, d: f64, text: &str, w: Emu) -> usize {
        let size = self.size_of(style, d);
        let bold = self
            .styles
            .get(style)
            .and_then(|s| s.get("bold"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let per_char = size * if bold { 0.56 } else { 0.5 } * PT as f64;
        let per_line = (w.max(1) as f64 / per_char).floor().max(1.0);
        text.split('\n')
            .map(|l| {
                let n = l.chars().count() as f64;
                ((n / per_line).ceil() as usize).max(1)
            })
            .sum()
    }
}

fn default_content(size: SlideSize) -> [Emu; 4] {
    let m = (0.75 * IN as f64) as Emu;
    let top = (2.2 * IN as f64) as Emu;
    let bottom = (0.85 * IN as f64) as Emu;
    [m, top, size.cx - 2 * m, (size.cy - top - bottom).max(IN)]
}

/// Keys a named style may set.
pub const STYLE_DEF_KEYS: &[&str] = &[
    "font",
    "size",
    "bold",
    "italic",
    "underline",
    "strike",
    "color",
    "caps",
    "letter_spacing",
    "align",
    "line_spacing",
    "space_before",
    "space_after",
];
const RUN_STYLE_KEYS: &[&str] = &[
    "font",
    "size",
    "bold",
    "italic",
    "underline",
    "strike",
    "color",
    "caps",
    "letter_spacing",
];
pub const HEADER_KEYS: &[&str] = &["x", "y", "w", "gap", "content_gap", "rule", "align"];
pub const FOOTER_KEYS: &[&str] = &[
    "text",
    "page_numbers",
    "logo",
    "logo_h",
    "bar",
    "note_field",
    "y",
    "style",
    "note_style",
];
pub const COVER_KEYS: &[&str] = &["background", "x", "w", "rule", "logo", "logo_h", "align"];

// ── resolving styles and palette names ────────────────────────────────────

/// Keys whose string values may be palette names.
const COLOR_KEYS: &[&str] = &[
    "color",
    "fill",
    "line",
    "border",
    "header_fill",
    "header_color",
    "band_fill",
    "background",
];

/// What level of a text a `style` is applied at.
#[derive(Clone, Copy, PartialEq)]
enum Level {
    Element,
    Para,
    Run,
}

fn apply_style(o: &mut Map<String, Value>, look: &Look, level: Level) -> Result<()> {
    let Some(name) = o.remove("style") else {
        return Ok(());
    };
    let name = name
        .as_str()
        .context("`style` must be the name of a text style")?
        .to_owned();
    let Some(st) = look.styles.get(&name).and_then(Value::as_object) else {
        let mut names: Vec<&str> = look.styles.keys().map(String::as_str).collect();
        names.sort_unstable();
        bail!(
            "there is no text style \"{name}\" (styles: {})",
            names.join(", ")
        );
    };
    for (k, v) in st {
        if level == Level::Run && !RUN_STYLE_KEYS.contains(&k.as_str()) {
            continue;
        }
        if !o.contains_key(k) {
            o.insert(k.clone(), v.clone());
        }
    }
    // `heading` / `body`: the theme's fonts.
    if let Some(Value::String(f)) = o.get_mut("font") {
        match f.as_str() {
            "heading" => *f = "+mj-lt".into(),
            "body" => *f = "+mn-lt".into(),
            _ => {}
        }
    }
    Ok(())
}

/// Palette names in colour keys → colours.
fn paint(v: &mut Value, look: &Look) {
    match v {
        Value::Object(o) => {
            for (k, x) in o.iter_mut() {
                if COLOR_KEYS.contains(&k.as_str())
                    && let Value::String(s) = x
                    && let Some(c) = look.color(s)
                {
                    *x = c;
                    continue;
                }
                if k == "stops"
                    && let Value::Array(a) = x
                {
                    for s in a.iter_mut() {
                        if let Value::String(n) = s
                            && let Some(c) = look.color(n)
                        {
                            *s = c;
                        }
                    }
                }
                paint(x, look);
            }
        }
        Value::Array(a) => {
            for x in a {
                paint(x, look);
            }
        }
        _ => {}
    }
    if let Value::Object(o) = v
        && let Some(Value::String(f)) = o.get_mut("font")
    {
        match f.as_str() {
            "heading" => *f = "+mj-lt".into(),
            "body" => *f = "+mn-lt".into(),
            _ => {}
        }
    }
}

fn resolve_text(t: &mut Value, look: &Look) -> Result<()> {
    let Value::Array(paras) = t else {
        return Ok(());
    };
    for p in paras {
        if let Value::Object(po) = p {
            apply_style(po, look, Level::Para)?;
            if let Some(Value::Array(runs)) = po.get_mut("runs") {
                for r in runs {
                    if let Value::Object(ro) = r {
                        apply_style(ro, look, Level::Run)?;
                    }
                }
            }
        }
    }
    Ok(())
}

/// An element with its `style` names and palette colours written out.
pub fn resolve(v: &Value, cx: &Ctx) -> Result<Value> {
    let look = look_of(cx);
    let look: &Look = &look;
    let mut v = v.clone();
    if let Value::Object(o) = &mut v {
        let ty = o
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        if o.contains_key("style") && !matches!(ty.as_str(), "shape" | "text" | "table") {
            bail!("`style` is for shapes, text boxes and tables");
        }
        apply_style(o, look, Level::Element)?;
        if ty == "table" {
            o.remove("line_spacing");
            o.remove("space_before");
            o.remove("space_after");
            if let Some(Value::Array(rows)) = o.get_mut("rows") {
                for row in rows {
                    if let Value::Array(cells) = row {
                        for c in cells {
                            if let Value::Object(co) = c {
                                apply_style(co, look, Level::Element)?;
                                if let Some(t) = co.get_mut("text") {
                                    resolve_text(t, look)?;
                                }
                            } else if let Value::Array(_) = c {
                                resolve_text(c, look)?;
                            }
                        }
                    }
                }
            }
        } else if let Some(t) = o.get_mut("text") {
            resolve_text(t, look)?;
        }
    }
    paint(&mut v, look);
    Ok(v)
}

/// A slide background with palette names written out.
pub fn resolve_background(v: &Value, cx: &Ctx) -> Value {
    match (&cx.look, v) {
        (Some(look), Value::String(s)) => look.color(s).unwrap_or_else(|| v.clone()),
        (Some(look), _) => {
            let mut w = json!({"background": v});
            paint(&mut w, look);
            w["background"].take()
        }
        _ => v.clone(),
    }
}

// ── transitions ────────────────────────────────────────────────────────────

/// `"fade"`, `"none"`, or `{"type", "duration", "direction",
/// "orientation", "through_black", "advance_after"}`.
pub fn transition(v: &Value) -> Result<Option<TransitionSpec>> {
    let (ty, o) = match v {
        Value::Bool(false) => return Ok(None),
        Value::String(s) => (s.as_str(), None),
        Value::Object(_) => {
            let o = object(
                v,
                "`transition`",
                &[
                    "type",
                    "duration",
                    "direction",
                    "orientation",
                    "through_black",
                    "advance_after",
                ],
            )?;
            (opt_str(o, "type")?.unwrap_or("fade"), Some(o))
        }
        _ => bail!("`transition` must be a type such as \"fade\", an object, or \"none\""),
    };
    let kind = match ty {
        "none" => return Ok(None),
        "uncover" => "pull",
        t if slides_engine::author::TRANSITIONS.contains(&t) => t,
        other => bail!(
            "transition `type` \"{other}\" is not one of none, cut, fade, push, wipe, split, cover, uncover, zoom"
        ),
    };
    let mut t = TransitionSpec {
        kind: kind.to_owned(),
        ..TransitionSpec::default()
    };
    let secs = |v: &Value, key: &str| -> Result<u32> {
        let s = match v {
            Value::Number(n) => n.as_f64().unwrap_or(f64::NAN),
            Value::String(s) => {
                let s = s.trim();
                if let Some(ms) = s.strip_suffix("ms") {
                    ms.trim().parse::<f64>().unwrap_or(f64::NAN) / 1000.0
                } else {
                    s.trim_end_matches('s')
                        .trim()
                        .parse::<f64>()
                        .unwrap_or(f64::NAN)
                }
            }
            _ => f64::NAN,
        };
        if !(0.0..=3600.0).contains(&s) {
            bail!("`{key}` must be seconds (0.7) or \"700ms\"");
        }
        Ok((s * 1000.0).round() as u32)
    };
    if let Some(o) = o {
        if let Some(d) = o.get("duration").filter(|d| !d.is_null()) {
            t.duration_ms = Some(secs(d, "duration")?.max(1));
        }
        if let Some(a) = o.get("advance_after").filter(|d| !d.is_null()) {
            t.advance_after_ms = Some(secs(a, "advance_after")?);
        }
        t.through_black = opt_bool(o, "through_black")?.unwrap_or(false);
        if let Some(d) = opt_str(o, "direction")? {
            let side = match d {
                "left" | "l" => "l",
                "right" | "r" => "r",
                "up" | "u" => "u",
                "down" | "d" => "d",
                "in" => "in",
                "out" => "out",
                other => bail!(
                    "transition `direction` must be left, right, up, down (push, wipe, cover, uncover) or in, out (split, zoom), not \"{other}\""
                ),
            };
            let ok = match kind {
                "split" | "zoom" => matches!(side, "in" | "out"),
                "push" | "wipe" | "cover" | "pull" => !matches!(side, "in" | "out"),
                _ => false,
            };
            if !ok {
                bail!("a {ty} transition has no direction \"{d}\"");
            }
            t.dir = Some(side.to_owned());
        }
        if let Some(or) = opt_str(o, "orientation")? {
            if kind != "split" {
                bail!("`orientation` is for split transitions");
            }
            t.orient = Some(
                match or {
                    "horizontal" | "horz" => "horz",
                    "vertical" | "vert" => "vert",
                    other => bail!("`orientation` must be horizontal or vertical, not \"{other}\""),
                }
                .to_owned(),
            );
        }
    }
    Ok(Some(t))
}

// ── slide furniture ────────────────────────────────────────────────────────

/// A length from a furniture setting, else `d`.
fn len_or(o: &Map<String, Value>, key: &str, cx: &Ctx, d: Emu) -> Result<Emu> {
    match o.get(key).filter(|v| !v.is_null()) {
        Some(v) => length(v, cx, key),
        None => Ok(d),
    }
}

/// A text box element (as JSON) for furniture.
fn text_box(name: &str, x: Emu, y: Emu, w: Emu, h: Emu, style: &str, text: Value) -> Value {
    json!({"type": "text", "name": name, "x": e(x), "y": e(y), "w": e(w), "h": e(h),
           "insets": 0, "anchor": "top", "style": style, "text": text})
}

/// What a slide's header puts on it, and the content area under it.
pub struct Furniture {
    pub elements: Vec<Element>,
    pub content: [Emu; 4],
}

fn title_element(v: Value, cx: &Ctx, ph: &str) -> Result<Element> {
    let mut el = element(&v, cx)?;
    if let ElementKind::Shape(s) = &mut el.kind {
        s.placeholder = Some(ph.to_owned());
    }
    Ok(el)
}

/// The header of a slide with `eyebrow`, `title` and `subtitle` (the
/// title is the slide's title placeholder).
pub fn header(so: &Map<String, Value>, cx: &Ctx, look: &Look) -> Result<Furniture> {
    let h = look.header.clone().unwrap_or_default();
    let size = look.size;
    let margin = (0.75 * IN as f64) as Emu;
    let x = len_or(&h, "x", cx, margin)?;
    let w = len_or(&h, "w", cx, size.cx - 2 * x)?;
    let mut y = len_or(&h, "y", cx, (0.55 * IN as f64) as Emu)?;
    let gap = len_or(&h, "gap", cx, (0.06 * IN as f64) as Emu)?;
    let content_gap = len_or(&h, "content_gap", cx, (0.4 * IN as f64) as Emu)?;
    let align = opt_str(&h, "align")?;
    let mut out = Vec::new();
    let with_align = |mut v: Value| {
        if let Some(a) = align {
            v["align"] = json!(a);
        }
        v
    };
    if let Some(r) = h.get("rule").filter(|r| r.is_object()) {
        let ro = object(r, "header `rule`", &["color", "w", "h"])?;
        let rh = len_or(ro, "h", cx, (0.06 * IN as f64) as Emu)?;
        let rw = len_or(ro, "w", cx, (0.9 * IN as f64) as Emu)?;
        let rx = if align == Some("center") {
            x + (w - rw) / 2
        } else {
            x
        };
        let color = ro.get("color").cloned().unwrap_or(json!("highlight"));
        out.push(element(
            &resolve(
                &json!({"type": "shape", "name": "Header rule", "x": e(rx), "y": e(y), "w": e(rw), "h": e(rh), "fill": color, "line": "none"}),
                cx,
            )?,
            cx,
        )?);
        y += rh + (0.18 * IN as f64) as Emu;
    }
    if let Some(t) = opt_str(so, "eyebrow")? {
        let lh = look.line_h("eyebrow", 11.0);
        let v = with_align(text_box("Eyebrow", x, y, w, lh, "eyebrow", json!(t)));
        out.push(element(&resolve(&v, cx)?, cx)?);
        y += lh + gap;
    }
    if let Some(t) = opt_str(so, "title")? {
        let lines = look.lines("title", 30.0, t, w).min(4);
        let lh = look.line_h("title", 30.0) * lines as Emu;
        let v = with_align(text_box("Title", x, y, w, lh, "title", json!(t)));
        out.push(title_element(resolve(&v, cx)?, cx, "title")?);
        y += lh + gap;
    }
    if let Some(t) = opt_str(so, "subtitle")? {
        let lines = look.lines("subtitle", 15.0, t, w).min(4);
        let lh = look.line_h("subtitle", 15.0) * lines as Emu;
        let v = with_align(text_box("Subtitle", x, y, w, lh, "subtitle", json!(t)));
        out.push(element(&resolve(&v, cx)?, cx)?);
        y += lh;
    }
    let mut content = look.content;
    let top = y + content_gap;
    if top > content[1] {
        let bottom = content[1] + content[3];
        content[1] = top;
        content[3] = (bottom - top).max(IN / 2);
    }
    Ok(Furniture {
        elements: out,
        content,
    })
}

/// A cover or section slide: a dark background, a rule, eyebrow, a big
/// title and subtitle centred on the slide's height.
pub fn cover(so: &Map<String, Value>, cx: &Ctx, look: &Look, section: bool) -> Result<Furniture> {
    let c = &look.cover;
    let size = look.size;
    let margin = (0.75 * IN as f64) as Emu;
    let x = len_or(c, "x", cx, margin)?;
    let w = len_or(c, "w", cx, ((size.cx - 2 * x) as f64 * 0.75) as Emu)?;
    let align = opt_str(c, "align")?;
    let eyebrow = opt_str(so, "eyebrow")?;
    let title = opt_str(so, "title")?;
    let subtitle = opt_str(so, "subtitle")?;
    let rule_h = (0.07 * IN as f64) as Emu;
    let gap = (0.14 * IN as f64) as Emu;
    let title_lines = title.map_or(0, |t| look.lines("cover_title", 44.0, t, w).min(5));
    let sub_lines = subtitle.map_or(0, |t| look.lines("cover_subtitle", 17.0, t, w).min(5));
    let eh = look.line_h("cover_eyebrow", 12.0);
    let th = look.line_h("cover_title", 44.0) * title_lines as Emu;
    let sh = look.line_h("cover_subtitle", 17.0) * sub_lines as Emu;
    let rule = c.get("rule").cloned().unwrap_or(json!({}));
    let has_rule = !matches!(rule, Value::Bool(false));
    let mut total = 0;
    if has_rule {
        total += rule_h + gap * 2;
    }
    if eyebrow.is_some() {
        total += eh + gap;
    }
    total += th;
    if subtitle.is_some() {
        total += gap * 2 + sh;
    }
    let mut y = ((size.cy as f64 * if section { 0.5 } else { 0.47 }) - total as f64 / 2.0) as Emu;
    let mut out = Vec::new();
    let with_align = |mut v: Value| {
        if let Some(a) = align {
            v["align"] = json!(a);
        }
        v
    };
    if has_rule {
        let ro = match &rule {
            Value::Object(_) => object(&rule, "cover `rule`", &["color", "w", "h"])?.clone(),
            _ => Map::new(),
        };
        let rw = len_or(&ro, "w", cx, (0.9 * IN as f64) as Emu)?;
        let rh = len_or(&ro, "h", cx, rule_h)?;
        let rx = if align == Some("center") {
            x + (w - rw) / 2
        } else {
            x
        };
        let color = ro.get("color").cloned().unwrap_or(json!("highlight"));
        out.push(element(
            &resolve(
                &json!({"type": "shape", "name": "Cover rule", "x": e(rx), "y": e(y), "w": e(rw), "h": e(rh), "fill": color, "line": "none"}),
                cx,
            )?,
            cx,
        )?);
        y += rh + gap * 2;
    }
    if let Some(t) = eyebrow {
        let v = with_align(text_box("Eyebrow", x, y, w, eh, "cover_eyebrow", json!(t)));
        out.push(element(&resolve(&v, cx)?, cx)?);
        y += eh + gap;
    }
    if let Some(t) = title {
        let v = with_align(text_box("Title", x, y, w, th, "cover_title", json!(t)));
        out.push(title_element(resolve(&v, cx)?, cx, "ctrTitle")?);
        y += th;
    }
    if let Some(t) = subtitle {
        y += gap * 2;
        let v = with_align(text_box(
            "Subtitle",
            x,
            y,
            w,
            sh,
            "cover_subtitle",
            json!(t),
        ));
        out.push(element(&resolve(&v, cx)?, cx)?);
        y += sh;
    }
    if let Some(logo) = opt_str(c, "logo")? {
        let lh = len_or(c, "logo_h", cx, (0.42 * IN as f64) as Emu)?;
        out.push(element(
            &json!({"type": "picture", "name": "Logo", "path": logo, "x": e(x), "y": e(size.cy - margin - lh), "h": e(lh)}),
            cx,
        )?);
    }
    let content = [
        x,
        y + IN / 3,
        size.cx - x - margin,
        (size.cy - y - IN).max(IN / 2),
    ];
    Ok(Furniture {
        elements: out,
        content,
    })
}

/// The cover's background (palette names allowed), if any.
pub fn cover_background(look: &Look, cx: &Ctx, section: bool) -> Value {
    let bg = look
        .cover
        .get("background")
        .cloned()
        .unwrap_or(json!(if section { "primary" } else { "dark" }));
    resolve_background(&bg, cx)
}

/// The footer of slide number `n`: bar, logo, text, the slide's note and
/// its number (a slide number field).
pub fn footer(so: &Map<String, Value>, cx: &Ctx, look: &Look) -> Result<Vec<Element>> {
    let Some(f) = &look.footer else {
        if get_str(so, "footer_note").is_some() {
            bail!("`footer_note` needs a deck `footer` (or a preset)");
        }
        return Ok(Vec::new());
    };
    let size = look.size;
    let margin = (0.75 * IN as f64) as Emu;
    let row_h = (0.25 * IN as f64) as Emu;
    let row_y = len_or(f, "y", cx, size.cy - (0.5 * IN as f64) as Emu)?;
    let style = opt_str(f, "style")?.unwrap_or("footer").to_owned();
    let note_style = opt_str(f, "note_style")?.unwrap_or("note").to_owned();
    let mut out = Vec::new();
    match f.get("bar") {
        None | Some(Value::Null) | Some(Value::Bool(false)) => {}
        Some(b) => {
            let bo = match b {
                Value::Object(_) => object(b, "footer `bar`", &["color", "height"])?.clone(),
                Value::Bool(true) => Map::new(),
                _ => bail!("footer `bar` must be {{color, height}} or false"),
            };
            let bh = len_or(&bo, "height", cx, (0.08 * IN as f64) as Emu)?;
            let color = bo.get("color").cloned().unwrap_or(json!("primary"));
            out.push(element(
                &resolve(
                    &json!({"type": "shape", "name": "Footer bar", "x": 0, "y": e(size.cy - bh), "w": e(size.cx), "h": e(bh), "fill": color, "line": "none"}),
                    cx,
                )?,
                cx,
            )?);
        }
    }
    let mut x = margin;
    if let Some(logo) = opt_str(f, "logo")? {
        let lh = len_or(f, "logo_h", cx, (0.2 * IN as f64) as Emu)?;
        let el = element(
            &json!({"type": "picture", "name": "Logo", "path": logo, "x": e(margin), "y": e(row_y + (row_h - lh) / 2), "h": e(lh)}),
            cx,
        )?;
        x += el.xfrm.cx + (0.2 * IN as f64) as Emu;
        out.push(el);
    }
    let num_w = IN;
    let right = size.cx - margin;
    let numbers = opt_bool(f, "page_numbers")?.unwrap_or(true);
    let text_end = if let Some(t) = opt_str(f, "text")? {
        let chars = t.chars().count() as f64;
        let est = (chars * look.size_of(&style, 10.0) * 0.52 * PT as f64) as Emu + IN / 4;
        let w = est.min(right - num_w - x);
        let v = text_box("Footer", x, row_y, w, row_h, &style, json!(t));
        let mut v = v;
        v["anchor"] = json!("middle");
        v["wrap"] = json!(false);
        out.push(element(&resolve(&v, cx)?, cx)?);
        x + w
    } else {
        x
    };
    let notes_on = opt_bool(f, "note_field")?.unwrap_or(true);
    if let Some(n) = get_str(so, "footer_note") {
        if !notes_on {
            bail!("the deck's footer has `note_field: false`");
        }
        let start = text_end + IN / 4;
        let end = right - if numbers { num_w + IN / 5 } else { 0 };
        if end - start > IN / 2 {
            let mut v = text_box(
                "Footer note",
                start,
                row_y,
                end - start,
                row_h,
                &note_style,
                json!(n),
            );
            v["anchor"] = json!("middle");
            v["align"] = json!("right");
            out.push(element(&resolve(&v, cx)?, cx)?);
        }
    }
    if numbers {
        let mut v = text_box(
            "Slide number",
            right - num_w,
            row_y,
            num_w,
            row_h,
            &style,
            json!([{"runs": [{"field": "slide_number"}]}]),
        );
        v["anchor"] = json!("middle");
        v["align"] = json!("right");
        out.push(element(&resolve(&v, cx)?, cx)?);
    }
    Ok(out)
}

fn get_str<'a>(o: &'a Map<String, Value>, k: &str) -> Option<&'a str> {
    o.get(k).and_then(Value::as_str)
}

/// The deck look for `cx` (the presentation's plain one for tool calls).
pub fn look_of(cx: &Ctx) -> Arc<Look> {
    match &cx.look {
        Some(l) => l.clone(),
        None => Arc::new(Look::fallback(cx.size.unwrap_or(SlideSize::WIDE))),
    }
}

// ── schema ─────────────────────────────────────────────────────────────────

fn r(name: &str) -> Value {
    json!({"$ref": format!("#/$defs/{name}")})
}

/// `$defs` entries: a named text style and a transition.
pub fn schema_defs() -> Vec<(&'static str, Value)> {
    vec![
        (
            "text_style",
            json!({
                "type": "object", "additionalProperties": false,
                "description": "A named text style: character and paragraph formatting. `font` may be heading or body (the theme's fonts), `color` a palette name.",
                "properties": {
                    "font": {"type": "string"}, "size": {"type": ["number", "string"]},
                    "bold": {"type": "boolean"}, "italic": {"type": "boolean"}, "underline": {"type": "boolean"}, "strike": {"type": "boolean"},
                    "color": r("color"), "caps": {"type": "boolean"}, "letter_spacing": {"type": ["number", "string"]},
                    "align": {"type": "string", "enum": ["left", "center", "right", "justify"]},
                    "line_spacing": {"type": ["number", "string"]}, "space_before": {"type": ["number", "string"]}, "space_after": {"type": ["number", "string"]},
                },
            }),
        ),
        (
            "transition",
            json!({
                "description": "How the slide comes in: a type, \"none\", or {type, duration (seconds or \"700ms\"), direction (left, right, up, down: the way slides move; in, out for split and zoom), orientation (horizontal, vertical: split), through_black (fade, cut), advance_after (seconds)}.",
                "anyOf": [
                    {"type": "string", "enum": ["none", "cut", "fade", "push", "wipe", "split", "cover", "uncover", "zoom"]},
                    {"type": "boolean", "const": false},
                    {"type": "object", "additionalProperties": false, "properties": {
                        "type": {"type": "string", "enum": ["none", "cut", "fade", "push", "wipe", "split", "cover", "uncover", "zoom"]},
                        "duration": {"type": ["number", "string"]},
                        "direction": {"type": "string", "enum": ["left", "right", "up", "down", "in", "out"]},
                        "orientation": {"type": "string", "enum": ["horizontal", "vertical"]},
                        "through_black": {"type": "boolean"},
                        "advance_after": {"type": ["number", "string"]},
                    }},
                ],
            }),
        ),
    ]
}

/// Deck keys for the look.
pub fn deck_props() -> Map<String, Value> {
    let rule = json!({"description": "An accent bar above the eyebrow: {color, w, h}, or false.", "anyOf": [
        {"type": "boolean", "const": false},
        {"type": "object", "additionalProperties": false, "properties": {"color": r("color"), "w": r("length"), "h": r("length")}}]});
    let palette: Map<String, Value> = PALETTE
        .iter()
        .map(|n| ((*n).to_owned(), json!({"type": "string"})))
        .collect();
    json!({
        "preset": {"type": "string", "enum": PRESETS, "description": "A built-in look: theme colours and fonts, palette, text styles, header and footer. The deck's own settings go over it."},
        "palette": {"type": "object", "additionalProperties": false, "properties": palette,
            "description": "Named colours used by styles and components (and allowed wherever a colour is): #RRGGBB or a theme colour. Default from the theme: primary accent1, highlight accent2, ink tx1, dark tx2, paper bg2, white bg1, muted/divider/tint mixed from them."},
        "styles": {"type": "object", "additionalProperties": r("text_style"),
            "description": "Named text styles (used with `style`): eyebrow, title, subtitle, body, small, note, label, card_title, stat, number, badge, callout, table, table_header, footer, cover_eyebrow, cover_title, cover_subtitle, and any of your own. Keys given here go over the preset's."},
        "header": {"description": "Where a slide's eyebrow, title and subtitle go (a preset turns it on; false: none). Slides with a header put `title` in a title placeholder, and components default to the area under it.",
            "anyOf": [{"type": "boolean"}, {"type": "object", "additionalProperties": false, "properties": {
                "x": r("length"), "y": {"description": "Top of the header (default 0.55in).", "allOf": [r("length")]}, "w": r("length"),
                "gap": r("length"), "content_gap": {"description": "Space between the header and the content area.", "allOf": [r("length")]},
                "rule": rule.clone(), "align": {"type": "string", "enum": ["left", "center"]}}}]},
        "footer": {"description": "Each slide's footer (cover and section slides have none unless `footer: true`).",
            "anyOf": [{"type": "boolean"}, {"type": "object", "additionalProperties": false, "properties": {
                "text": {"type": "string", "description": "e.g. \"Prepared for …\"."},
                "page_numbers": {"type": "boolean", "description": "A slide number field at the right (default true)."},
                "logo": {"type": "string", "description": "A PNG or JPEG at the left."},
                "logo_h": r("length"),
                "bar": {"description": "A bar along the bottom edge: {color, height}, or false.", "anyOf": [{"type": "boolean"},
                    {"type": "object", "additionalProperties": false, "properties": {"color": r("color"), "height": r("length")}}]},
                "note_field": {"type": "boolean", "description": "Show slides' `footer_note` (default true)."},
                "y": r("length"), "style": {"type": "string"}, "note_style": {"type": "string"}}}]},
        "cover": {"type": "object", "additionalProperties": false, "description": "Cover and section slides (`kind`).", "properties": {
            "background": r("background"), "x": r("length"), "w": r("length"), "rule": rule,
            "logo": {"type": "string"}, "logo_h": r("length"), "align": {"type": "string", "enum": ["left", "center"]}}},
        "content": {"type": "object", "additionalProperties": false, "description": "The content area under the header (default 0.75in margins, from 2.2in to 0.85in above the bottom) and the gap between cards.",
            "properties": {"x": r("length"), "y": r("length"), "w": r("length"), "h": r("length"), "gap": r("length")}},
        "transition": {"description": "Every slide's transition unless it has its own.", "allOf": [r("transition")]},
    })
    .as_object()
    .cloned()
    .unwrap_or_default()
}

/// Slide keys for the look.
pub fn slide_props() -> Map<String, Value> {
    json!({
        "kind": {"type": "string", "enum": ["content", "cover", "section"], "description": "cover / section: a dark slide with a big title (see the deck's `cover`)."},
        "eyebrow": {"type": "string", "description": "The small label above the title (needs a header or preset)."},
        "subtitle": {"type": "string"},
        "footer": {"type": "boolean", "description": "false hides the deck's footer on this slide."},
        "footer_note": {"type": "string", "description": "Text in the footer, e.g. a source."},
        "transition": r("transition"),
    })
    .as_object()
    .cloned()
    .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[std::prelude::v1::test]
    fn presets_styles_and_palette_resolve() {
        for name in PRESETS {
            let v = preset(name).unwrap();
            assert!(v["theme"]["colors"]["accent1"].is_string(), "{name}");
        }
        assert!(preset("nope").is_err());
        let p = Presentation::new();
        let deck = json!({"preset": "consulting-navy", "styles": {"eyebrow": {"size": 12}}, "palette": {"muted": "#123456"}});
        let look = Look::build(&p, deck.as_object().unwrap(), &Ctx::default()).unwrap();
        assert_eq!(look.size_of("eyebrow", 0.0), 12.0);
        assert_eq!(look.styles["eyebrow"]["caps"], true);
        let cx = Ctx {
            look: Some(Arc::new(look)),
            ..Ctx::default()
        };
        let v = resolve(
            &json!({"type": "text", "w": 1, "h": 1, "style": "eyebrow", "color": "muted",
                    "text": [{"text": "a", "style": "title"}, {"runs": [{"text": "b", "style": "small", "bold": false}]}]}),
            &cx,
        )
        .unwrap();
        assert_eq!(v["size"], 12);
        assert_eq!(v["color"], "#123456");
        assert_eq!(v["text"][0]["font"], "+mj-lt");
        assert_eq!(v["text"][0]["color"], "tx1");
        assert_eq!(v["text"][1]["runs"][0]["bold"], false);
        assert!(v["text"][1]["runs"][0].get("line_spacing").is_none());
        assert!(resolve(&json!({"type": "text", "style": "nope"}), &cx).is_err());
        assert!(resolve(&json!({"type": "line", "style": "title"}), &cx).is_err());
    }

    #[std::prelude::v1::test]
    fn reads_transitions() {
        let t = transition(&json!({"type": "push", "direction": "up", "duration": 1.2}))
            .unwrap()
            .unwrap();
        assert_eq!(
            (t.kind.as_str(), t.dir.as_deref(), t.duration_ms),
            ("push", Some("u"), Some(1200))
        );
        assert_eq!(transition(&json!("uncover")).unwrap().unwrap().kind, "pull");
        assert!(transition(&json!("none")).unwrap().is_none());
        assert!(transition(&json!({"type": "fade", "direction": "up"})).is_err());
        assert!(transition(&json!({"type": "spin"})).is_err());
        assert_eq!(
            transition(&json!({"type": "split", "orientation": "vertical", "direction": "in", "duration": "300ms"}))
                .unwrap()
                .unwrap()
                .duration_ms,
            Some(300)
        );
    }
}
