//! A deck's look: built-in presets (`"preset": "editorial"`), named
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
pub const PRESETS: [&str; 3] = ["editorial", "swiss", "mono"];

/// How cards, stats, notes, comparisons and tables are drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CardStyle {
    /// No boxes: a rule on top, numbers in the heading font, hairline tables.
    Rule,
    /// Solid square panels, the highlighted one in the accent colour.
    Fill,
}

impl CardStyle {
    fn parse(s: &str) -> Result<Self> {
        Ok(match s {
            "rule" => Self::Rule,
            "fill" => Self::Fill,
            other => bail!("`card_style` must be rule or fill, not \"{other}\""),
        })
    }
}

/// Text styles every deck has (presets and decks change or add to them).
/// Colours are palette names, `heading` / `body` the theme's fonts.
fn base_styles() -> Value {
    json!({
        "eyebrow": {"size": 11, "color": "muted"},
        "page_number": {"size": 10, "bold": true, "color": "ink"},
        "title": {"font": "heading", "size": 30, "bold": true, "color": "ink", "line_spacing": 1.0},
        "subtitle": {"size": 15, "color": "muted", "line_spacing": 1.1},
        "body": {"size": 16, "color": "ink", "line_spacing": 1.15},
        "small": {"size": 12, "color": "muted", "line_spacing": 1.2},
        "note": {"size": 9, "color": "muted"},
        "label": {"size": 10.5, "bold": true, "color": "muted"},
        "card_title": {"size": 16, "bold": true, "color": "ink", "line_spacing": 1.05},
        "stat": {"font": "heading", "size": 40, "bold": true, "color": "ink", "line_spacing": 1.0},
        "number": {"font": "heading", "size": 24, "bold": true, "color": "accent"},
        "numeral": {"font": "heading", "size": 30, "bold": true, "color": "accent", "line_spacing": 1.0},
        "badge": {"size": 11, "bold": true, "color": "on_accent"},
        "table": {"size": 12, "color": "ink"},
        "callout": {"size": 14, "color": "ink", "line_spacing": 1.15},
        "table_header": {"size": 11, "bold": true, "color": "on_accent"},
        "footer": {"size": 9, "color": "muted"},
        "cover_brand": {"size": 12, "bold": true, "color": "ink"},
        "cover_eyebrow": {"size": 12, "color": "accent"},
        "cover_title": {"font": "heading", "size": 44, "bold": true, "color": "ink", "line_spacing": 1.0},
        "cover_subtitle": {"size": 17, "color": "muted", "line_spacing": 1.2},
        "cover_meta": {"size": 11, "color": "muted"},
        "cover_mark": {"font": "heading", "size": 96, "color": "on_accent", "line_spacing": 0.9},
        "cover_mark_label": {"size": 11, "color": "on_accent"},
        "section_eyebrow": {"size": 11, "bold": true, "color": "on_dark_accent"},
        "section_title": {"font": "heading", "size": 40, "bold": true, "color": "on_dark", "line_spacing": 1.0},
        "section_subtitle": {"size": 17, "color": "on_dark_muted", "line_spacing": 1.2},
    })
}

/// The swiss look's furniture and styles (also `mono`'s).
fn swiss(name: &str, palette: Value) -> Value {
    const MONO: &str = "JetBrains Mono";
    json!({
        "theme": {"name": name, "fonts": {"heading": "Inter", "body": "Inter"}},
        "palette": palette,
        "card_style": "fill",
        "background": "background",
        "styles": {
            "eyebrow": {"font": MONO, "size": 10, "caps": true, "color": "muted"},
            "page_number": {"font": MONO, "size": 10, "bold": true, "color": "accent"},
            "title": {"size": 32, "bold": true},
            "subtitle": {"size": 15},
            "stat": {"size": 44, "bold": true},
            "number": {"font": MONO, "size": 10, "bold": true, "color": "accent"},
            "numeral": {"font": "heading", "size": 28, "bold": true},
            "badge": {"font": MONO, "size": 10, "bold": true},
            "label": {"font": MONO, "size": 10, "bold": false, "caps": true},
            "callout": {"size": 15, "color": "on_accent"},
            "table_header": {"size": 11, "bold": true, "color": "on_accent"},
            "footer": {"font": MONO, "size": 9, "caps": true},
            "cover_brand": {"font": MONO, "size": 10, "bold": true, "color": "on_accent"},
            "cover_eyebrow": {"font": MONO, "size": 10, "caps": true, "color": "accent_soft"},
            "cover_title": {"size": 54, "bold": true, "color": "on_accent", "line_spacing": 0.95},
            "cover_subtitle": {"color": "accent_soft"},
            "cover_meta": {"font": MONO, "size": 10, "caps": true, "color": "accent_soft"},
            "section_eyebrow": {"font": MONO, "size": 10, "caps": true, "bold": true, "color": "on_dark_muted"},
            "section_title": {"size": 40, "bold": true},
        },
        "header": {"y": "0.5in", "gap": "0.18in", "content_gap": "0.45in", "page_number": true,
                   "text": true, "rule": {"color": "ink", "w": "full", "h": "3pt", "gap": "0.1in"}},
        "footer": {"page_numbers": false, "y": "6.94in", "mark": {"color": "accent", "size": "0.14in"}},
        "cover": {"background": "accent", "y": "2.05in", "motif": "grid", "brand": true, "rule": false,
                  "section_background": "dark", "section_y": "0.9in"},
    })
}

/// A preset: theme, palette, styles, header, footer, cover, card style and
/// slide background.
pub fn preset(name: &str) -> Result<Value> {
    Ok(match name {
        "editorial" => json!({
            "theme": {"name": "Editorial", "fonts": {"heading": "Instrument Serif", "body": "Inter"},
                "colors": {"dk1": "#1E1B18", "lt1": "#FFFFFF", "dk2": "#1E1B18", "lt2": "#ECE3D5",
                    "accent1": "#B4532A", "accent2": "#2E5E4E", "accent3": "#6E655C", "accent4": "#CDBFAC",
                    "accent5": "#D9CEBF", "accent6": "#8A3E1F", "hlink": "#B4532A", "folHlink": "#6E655C"}},
            "palette": {"background": "#F6F1E9", "muted": "#6E655C", "line": "#D9CEBF", "divider": "#D9CEBF",
                "panel": "#ECE3D5", "paper": "#ECE3D5", "bar": "#CDBFAC", "accent_soft": "#F2DED0", "tint": "#F2DED0",
                "highlight": "accent1", "warning": "accent1", "good": "#2E5E4E", "success": "#2E5E4E",
                "bad": "#B4532A", "danger": "#B4532A", "dark": "#1E1B18", "on_dark": "#F6F1E9",
                "on_dark_accent": "#D9774A", "accent_light": "#D9774A", "on_dark_muted": "#B4A898"},
            "card_style": "rule",
            "background": "background",
            "styles": {
                "eyebrow": {"size": 11, "color": "muted"},
                "page_number": {"size": 9, "bold": true, "color": "ink"},
                "title": {"size": 40, "bold": false},
                "subtitle": {"size": 15},
                "stat": {"size": 54, "bold": false},
                "number": {"size": 30, "bold": false},
                "numeral": {"size": 34, "bold": false},
                "label": {"size": 11, "bold": true},
                "callout": {"font": "heading", "size": 20, "line_spacing": 1.1},
                "table_header": {"size": 11, "bold": false, "color": "muted"},
                "cover_eyebrow": {"size": 12, "color": "accent"},
                "cover_title": {"size": 64, "bold": false, "line_spacing": 0.92},
                "cover_mark": {"size": 104, "bold": false},
                "section_title": {"size": 46, "bold": false},
            },
            "header": {"y": "0.52in", "gap": "0.12in", "content_gap": "0.5in",
                       "rule": {"color": "accent", "w": "0.35in", "h": "1.5pt", "position": "left"}},
            "footer": {"y": "7.05in", "rule": {"color": "line", "width": "0.75pt"}},
            "cover": {"background": "background", "y": "2.05in", "motif": "block", "brand": true,
                      "rule": {"color": "accent", "w": "0.35in", "h": "1.5pt", "position": "left"},
                      "section_background": "dark", "section_y": "0.9in"},
        }),
        "swiss" => {
            let mut v = swiss(
                "Swiss",
                json!({"background": "#FFFFFF", "muted": "#5F6368", "line": "#DADCE0", "divider": "#DADCE0",
                    "panel": "#F1F2F4", "paper": "#F1F2F4", "bar": "#C4C7CC", "accent_soft": "#E3E8FD", "tint": "#E3E8FD",
                    "highlight": "tx1", "warning": "accent1", "good": "#14804A", "success": "#14804A",
                    "bad": "#D33A1F", "danger": "#D33A1F", "dark": "#111111", "on_dark": "#FFFFFF",
                    "on_dark_accent": "#8FA0F0", "accent_light": "#8FA0F0", "on_dark_muted": "#A6A9AE"}),
            );
            v["theme"]["colors"] = json!({"dk1": "#111111", "lt1": "#FFFFFF", "dk2": "#111111", "lt2": "#F1F2F4",
                "accent1": "#2340D8", "accent2": "#14804A", "accent3": "#D33A1F", "accent4": "#5F6368",
                "accent5": "#C4C7CC", "accent6": "#8FA0F0", "hlink": "#2340D8", "folHlink": "#5F6368"});
            v
        }
        "mono" => {
            let mut v = swiss(
                "Mono",
                json!({"background": "#FFFFFF", "muted": "#666666", "line": "#DDDDDD", "divider": "#DDDDDD",
                    "panel": "#F2F2F2", "paper": "#F2F2F2", "bar": "#C8C8C8", "accent_soft": "#D9D9D9", "tint": "#E6E6E6",
                    "highlight": "accent2", "warning": "accent2", "good": "#1F7A4D", "success": "#1F7A4D",
                    "bad": "#D33A1F", "danger": "#D33A1F", "dark": "#111111", "on_dark": "#FFFFFF",
                    "on_dark_accent": "#FF6B57", "accent_light": "#BBBBBB", "on_dark_muted": "#A8A8A8"}),
            );
            v["theme"]["colors"] = json!({"dk1": "#111111", "lt1": "#FFFFFF", "dk2": "#111111", "lt2": "#F2F2F2",
                "accent1": "#111111", "accent2": "#D33A1F", "accent3": "#1F7A4D", "accent4": "#666666",
                "accent5": "#C8C8C8", "accent6": "#999999", "hlink": "#111111", "folHlink": "#666666"});
            v["styles"]["page_number"]["color"] = json!("highlight");
            v["footer"]["mark"]["color"] = json!("highlight");
            v
        }
        other => bail!("`preset` \"{other}\" is not one of {}", PRESETS.join(", ")),
    })
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
    /// How cards, stats and tables are drawn.
    pub card_style: CardStyle,
    /// Every slide's background unless the deck or slide has one.
    pub background: Option<Value>,
    /// The theme's heading and body fonts (for guessing text widths).
    pub fonts: [String; 2],
}

/// Palette names (besides theme colour names, which always work).
pub const PALETTE: [&str; 28] = [
    "accent",
    "accent_soft",
    "background",
    "panel",
    "line",
    "bar",
    "good",
    "bad",
    "on_accent",
    "on_dark",
    "on_dark_accent",
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

/// Palette colours made from the accent: a deck with its own accent colour
/// gets them mixed from it instead of the preset's.
const ACCENT_TINTS: [&str; 4] = ["accent_soft", "tint", "on_dark_accent", "accent_light"];

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
        put("accent", accent1, Some("accent1"));
        put("accent_soft", mix(accent1, white, 0.85), None);
        put("background", white, Some("bg1"));
        put("panel", paper, Some("bg2"));
        put("line", mix(ink, white, 0.82), None);
        put("bar", mix(ink, white, 0.7), None);
        put("good", Rgb([0x2E, 0x8B, 0x57]), None);
        put("bad", Rgb([0xC2, 0x41, 0x2D]), None);
        put("on_accent", white, Some("bg1"));
        put("on_dark", white, Some("bg1"));
        put("on_dark_accent", mix(accent1, white, 0.35), None);
        // Preset, then deck palette. A deck with its own accent colour
        // keeps the tints made from it, not the preset's.
        let own_accent = deck
            .get("theme")
            .and_then(|t| t.get("colors"))
            .and_then(|c| c.get("accent1"))
            .is_some_and(|c| !c.is_null());
        for (src, is_preset) in [(preset.get("palette"), true), (deck.get("palette"), false)]
            .into_iter()
            .filter_map(|(v, p)| v.filter(|v| !v.is_null()).map(|v| (v, p)))
        {
            let o = object(src, "`palette`", &PALETTE)?;
            for (k, v) in o {
                if is_preset && own_accent && ACCENT_TINTS.contains(&k.as_str()) {
                    continue;
                }
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
        let card_style = match opt_str(deck, "card_style")?
            .or(preset.get("card_style").and_then(Value::as_str))
        {
            Some(c) => CardStyle::parse(c)?,
            None => CardStyle::Fill,
        };
        let background = preset.get("background").cloned();
        let theme = deck_theme(deck)?.unwrap_or(Value::Null);
        let font = |k: &str| theme["fonts"][k].as_str().unwrap_or("Calibri").to_owned();
        let fonts = [font("heading"), font("body")];
        Ok(Look {
            fonts,
            card_style,
            background,
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

    /// The look for a dark slide (sections): text in the light colours,
    /// rules and panels mixed from the dark one.
    pub fn on_dark(&self, bg: &str) -> Look {
        let mut l = self.clone();
        let dark = self.rgb(bg);
        let light = self.rgb("on_dark");
        let set = |l: &mut Look, k: &str, c: Rgb| {
            l.palette.insert(k.to_owned(), (c, None));
        };
        let muted = self.rgb("on_dark_muted");
        for k in ["ink", "black"] {
            set(&mut l, k, light);
        }
        set(&mut l, "muted", muted);
        for k in ["line", "divider"] {
            set(&mut l, k, mix(dark, light, 0.28));
        }
        for k in ["panel", "paper"] {
            set(&mut l, k, mix(dark, light, 0.1));
        }
        set(&mut l, "bar", mix(dark, light, 0.4));
        set(&mut l, "background", dark);
        l
    }

    /// Whether a palette colour (or `#RRGGBB`) is dark.
    pub fn is_dark(&self, name: &str) -> bool {
        let Rgb([r, g, b]) = self.rgb(name);
        0.2126 * r as f64 + 0.7152 * g as f64 + 0.0722 * b as f64 <= 110.0
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
        let font = match self
            .styles
            .get(style)
            .and_then(|s| s.get("font"))
            .and_then(Value::as_str)
        {
            Some("heading") => self.fonts[0].as_str(),
            Some("body") | None => self.fonts[1].as_str(),
            Some(f) => f,
        };
        // Condensed display faces fit more on a line.
        let narrow = font.contains("Instrument Serif");
        let per_char =
            size * match (narrow, bold) {
                (true, _) => 0.4,
                (false, true) => 0.56,
                (false, false) => 0.5,
            } * PT as f64;
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
pub const HEADER_KEYS: &[&str] = &[
    "x",
    "y",
    "w",
    "gap",
    "content_gap",
    "rule",
    "align",
    "page_number",
    "separator",
    "text",
];
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
    "rule",
    "mark",
];
pub const COVER_KEYS: &[&str] = &[
    "background",
    "x",
    "y",
    "w",
    "rule",
    "logo",
    "logo_h",
    "align",
    "motif",
    "block_w",
    "brand",
    "section_background",
    "section_y",
];

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

/// The page number as shown: `02`.
fn page_text(cx: &Ctx) -> String {
    cx.slide_no
        .map_or_else(|| "#".to_owned(), |n| format!("{n:02}"))
}

/// A slide number field run in `style`.
fn page_run(cx: &Ctx, style: &str) -> Value {
    json!({"field": "slide_number", "text": page_text(cx), "style": style})
}

/// A filled rectangle (rules, blocks, squares).
fn rect(name: &str, x: Emu, y: Emu, w: Emu, h: Emu, fill: Value, cx: &Ctx) -> Result<Element> {
    element(
        &resolve(
            &json!({"type": "shape", "name": name, "x": e(x), "y": e(y), "w": e(w.max(1)), "h": e(h.max(1)), "fill": fill, "line": "none"}),
            cx,
        )?,
        cx,
    )
}

/// A short rule: `{color, w, h, position, gap}` (`w` may be "full").
struct Rule {
    color: Value,
    w: Emu,
    h: Emu,
    /// Beside the label (left of it) rather than above it.
    beside: bool,
    gap: Emu,
}

fn rule_of(v: Option<&Value>, what: &str, cx: &Ctx, full: Emu, on: bool) -> Result<Option<Rule>> {
    let o = match v {
        Some(Value::Bool(false)) => return Ok(None),
        None | Some(Value::Null) if !on => return Ok(None),
        None | Some(Value::Null) | Some(Value::Bool(true)) => Map::new(),
        Some(r @ Value::Object(_)) => {
            object(r, what, &["color", "w", "h", "position", "gap"])?.clone()
        }
        Some(_) => bail!("{what} must be {{color, w, h, position, gap}} or false"),
    };
    let w = match o.get("w") {
        Some(Value::String(s)) if s == "full" => full,
        _ => len_or(&o, "w", cx, (0.9 * IN as f64) as Emu)?,
    };
    let beside = match opt_str(&o, "position")?.unwrap_or("above") {
        "above" => false,
        "left" => true,
        other => bail!("{what} `position` must be above or left, not \"{other}\""),
    };
    Ok(Some(Rule {
        color: o.get("color").cloned().unwrap_or(json!("accent")),
        w,
        h: len_or(&o, "h", cx, (0.06 * IN as f64) as Emu)?,
        beside,
        gap: len_or(&o, "gap", cx, (0.18 * IN as f64) as Emu)?,
    }))
}

/// The header of a slide with `eyebrow`, `title` and `subtitle` (the
/// title is the slide's title placeholder): a rule above the label row or
/// beside it, the label row (page number, eyebrow, the deck's name at the
/// right), then the title and subtitle.
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
    let numbered = opt_bool(&h, "page_number")?.unwrap_or(false);
    let sep = opt_str(&h, "separator")?.unwrap_or(" / ").to_owned();
    let side_text = match h.get("text") {
        Some(Value::String(s)) => Some(s.clone()),
        Some(Value::Bool(true)) => look
            .footer
            .as_ref()
            .and_then(|f| get_str(f, "text"))
            .map(str::to_owned),
        None | Some(Value::Null) | Some(Value::Bool(false)) => None,
        Some(_) => bail!("header `text` must be a string or true (the footer's text)"),
    };
    let eyebrow = opt_str(so, "eyebrow")?;
    let has_row = eyebrow.is_some() || numbered || side_text.is_some();
    let lh = look.line_h("eyebrow", 11.0);
    let mut out = Vec::new();
    let with_align = |mut v: Value| {
        if let Some(a) = align {
            v["align"] = json!(a);
        }
        v
    };
    let mut label_x = x;
    if let Some(r) = rule_of(h.get("rule"), "header `rule`", cx, w, false)? {
        if r.beside && has_row {
            out.push(rect(
                "Header rule",
                x,
                y + (lh - r.h) / 2,
                r.w,
                r.h,
                r.color,
                cx,
            )?);
            label_x = x + r.w + (0.15 * IN as f64) as Emu;
        } else {
            let rx = if align == Some("center") {
                x + (w - r.w) / 2
            } else {
                x
            };
            out.push(rect("Header rule", rx, y, r.w, r.h, r.color, cx)?);
            y += r.h + r.gap;
        }
    }
    if has_row {
        let side_w = if side_text.is_some() { 3 * IN } else { 0 };
        if let Some(t) = &side_text {
            let mut v = text_box(
                "Header text",
                x + w - side_w,
                y,
                side_w,
                lh,
                "eyebrow",
                json!([{"runs": [{"text": t, "caps": false}]}]),
            );
            v["align"] = json!("right");
            v["wrap"] = json!(false);
            out.push(element(&resolve(&v, cx)?, cx)?);
        }
        let mut runs = Vec::new();
        if numbered {
            runs.push(page_run(cx, "page_number"));
            if eyebrow.is_some() {
                runs.push(json!({"text": sep, "style": "eyebrow"}));
            }
        }
        if let Some(t) = eyebrow {
            runs.push(json!({"text": t, "style": "eyebrow"}));
        }
        if !runs.is_empty() {
            let bw = (x + w - side_w - label_x).max(IN);
            let v = with_align(text_box(
                "Eyebrow",
                label_x,
                y,
                bw,
                lh,
                "eyebrow",
                json!([{"runs": runs}]),
            ));
            out.push(element(&resolve(&v, cx)?, cx)?);
        }
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

/// A cover or section slide. A cover: the deck's name (or cover logo) at
/// the top left, eyebrow (beside or under a short rule), a big title,
/// subtitle and a `meta` line at the bottom, with the cover's motif: an
/// accent `block` down the right carrying the slide's `mark` (a big number),
/// or a `grid` of squares. A section: eyebrow, title and subtitle on a dark
/// background, components under them. Without a `y` the text is centred on
/// the slide's height.
pub fn cover(so: &Map<String, Value>, cx: &Ctx, look: &Look, section: bool) -> Result<Furniture> {
    let c = &look.cover;
    let size = look.size;
    let margin = (0.75 * IN as f64) as Emu;
    let x = len_or(c, "x", cx, margin)?;
    let align = opt_str(c, "align")?;
    let motif = if section {
        "none"
    } else {
        opt_str(c, "motif")?.unwrap_or("none")
    };
    let mut out = Vec::new();
    // The motif and how wide the text may go beside it.
    let mut text_end = size.cx - margin;
    match motif {
        "none" => {}
        "block" => {
            let bw = len_or(c, "block_w", cx, (3.6 * IN as f64) as Emu)?;
            let bx = size.cx - bw;
            out.push(rect(
                "Cover block",
                bx,
                0,
                bw,
                size.cy,
                json!("accent"),
                cx,
            )?);
            let pad = (0.4 * IN as f64) as Emu;
            if let Some(t) = text_of(so, "mark_label")? {
                let lh = look.line_h("cover_mark_label", 11.0);
                let v = text_box(
                    "Cover mark label",
                    bx + pad,
                    (0.6 * IN as f64) as Emu,
                    bw - 2 * pad,
                    lh * 2,
                    "cover_mark_label",
                    json!(t),
                );
                out.push(element(&resolve(&v, cx)?, cx)?);
            }
            if let Some(t) = text_of(so, "mark")? {
                let mh = (look.size_of("cover_mark", 96.0) * 1.25 * PT as f64) as Emu;
                let mut v = text_box(
                    "Cover mark",
                    bx + pad * 3 / 4,
                    size.cy - (0.55 * IN as f64) as Emu - mh,
                    bw - pad,
                    mh,
                    "cover_mark",
                    json!(t),
                );
                v["anchor"] = json!("bottom");
                v["wrap"] = json!(false);
                out.push(element(&resolve(&v, cx)?, cx)?);
            }
            text_end = bx - (0.5 * IN as f64) as Emu;
        }
        "grid" => {
            let cell = (0.7 * IN as f64) as Emu;
            let pitch = (0.84 * IN as f64) as Emu;
            let gx = size.cx - margin - (0.36 * IN as f64) as Emu - 3 * pitch - cell;
            let gy = (size.cy as f64 * 0.233) as Emu;
            for r in 0..4 {
                for k in 0..4 {
                    let i = r * 4 + k;
                    let (fill, line) = match i {
                        1 | 6 | 11 => (json!("on_accent"), json!("none")),
                        9 => (json!("highlight"), json!("none")),
                        _ => (json!("none"), json!({"color": "on_accent", "width": "1pt"})),
                    };
                    let v = json!({"type": "shape", "name": format!("Cover square {}", i + 1),
                        "x": e(gx + k * pitch), "y": e(gy + r * pitch), "w": e(cell), "h": e(cell),
                        "fill": fill, "line": line});
                    out.push(element(&resolve(&v, cx)?, cx)?);
                }
            }
            text_end = gx - (0.45 * IN as f64) as Emu;
        }
        other => bail!("cover `motif` must be block, grid or none, not \"{other}\""),
    }
    let full = text_end - x;
    let w = if section {
        len_or(c, "w", cx, size.cx - margin - x)?
    } else {
        len_or(
            c,
            "w",
            cx,
            (((size.cx - 2 * x) as f64 * 0.75) as Emu).min(full),
        )?
    };
    let (s_eyebrow, s_title, s_sub) = if section {
        ("section_eyebrow", "section_title", "section_subtitle")
    } else {
        ("cover_eyebrow", "cover_title", "cover_subtitle")
    };
    let eyebrow = opt_str(so, "eyebrow")?;
    let title = opt_str(so, "title")?;
    let subtitle = opt_str(so, "subtitle")?;
    let gap = (0.14 * IN as f64) as Emu;
    let title_lines = title.map_or(0, |t| look.lines(s_title, 44.0, t, w).min(5));
    let sub_lines = subtitle.map_or(0, |t| look.lines(s_sub, 17.0, t, w).min(5));
    let eh = look.line_h(s_eyebrow, 12.0);
    let th = look.line_h(s_title, 44.0) * title_lines as Emu;
    let sh = look.line_h(s_sub, 17.0) * sub_lines as Emu;
    let rule = if section {
        None
    } else {
        rule_of(c.get("rule"), "cover `rule`", cx, w, true)?
    };
    let rule_above = rule
        .as_ref()
        .is_some_and(|r| !r.beside || eyebrow.is_none());
    let mut total = 0;
    if let Some(r) = rule.as_ref().filter(|_| rule_above) {
        total += r.h + gap * 2;
    }
    if eyebrow.is_some() {
        total += eh + gap;
    }
    total += th;
    if subtitle.is_some() {
        total += gap * 2 + sh;
    }
    let y_key = if section { "section_y" } else { "y" };
    let mut y = match c.get(y_key).filter(|v| !v.is_null()) {
        Some(v) => length(v, cx, y_key)?,
        None => ((size.cy as f64 * if section { 0.5 } else { 0.47 }) - total as f64 / 2.0) as Emu,
    };
    let with_align = |mut v: Value| {
        if let Some(a) = align {
            v["align"] = json!(a);
        }
        v
    };
    if !section {
        let top = (0.6 * IN as f64) as Emu;
        if let Some(logo) = opt_str(c, "logo")? {
            let lh = len_or(c, "logo_h", cx, (0.42 * IN as f64) as Emu)?;
            out.push(element(
                &json!({"type": "picture", "name": "Logo", "path": logo, "x": e(x), "y": e(top), "h": e(lh)}),
                cx,
            )?);
        } else if opt_bool(c, "brand")?.unwrap_or(false)
            && let Some(t) = look.footer.as_ref().and_then(|f| get_str(f, "text"))
        {
            let lh = look.line_h("cover_brand", 12.0);
            let v = text_box("Cover brand", x, top, full, lh, "cover_brand", json!(t));
            out.push(element(&resolve(&v, cx)?, cx)?);
        }
    }
    let mut label_x = x;
    if let Some(r) = &rule {
        if rule_above {
            let rx = if align == Some("center") {
                x + (w - r.w) / 2
            } else {
                x
            };
            out.push(rect("Cover rule", rx, y, r.w, r.h, r.color.clone(), cx)?);
            y += r.h + gap * 2;
        } else {
            out.push(rect(
                "Cover rule",
                x,
                y + (eh - r.h) / 2,
                r.w,
                r.h,
                r.color.clone(),
                cx,
            )?);
            label_x = x + r.w + (0.15 * IN as f64) as Emu;
        }
    }
    if let Some(t) = eyebrow {
        let v = with_align(text_box(
            "Eyebrow",
            label_x,
            y,
            w - (label_x - x),
            eh,
            s_eyebrow,
            json!(t),
        ));
        out.push(element(&resolve(&v, cx)?, cx)?);
        y += eh + gap;
    }
    if let Some(t) = title {
        let v = with_align(text_box("Title", x, y, w, th, s_title, json!(t)));
        out.push(title_element(resolve(&v, cx)?, cx, "ctrTitle")?);
        y += th;
    }
    if let Some(t) = subtitle {
        y += gap * 2;
        let v = with_align(text_box("Subtitle", x, y, w, sh, s_sub, json!(t)));
        out.push(element(&resolve(&v, cx)?, cx)?);
        y += sh;
    }
    if let Some(t) = text_of(so, "meta")? {
        let lh = look.line_h("cover_meta", 11.0);
        let v = with_align(text_box(
            "Cover meta",
            x,
            size.cy - (0.85 * IN as f64) as Emu,
            full,
            lh,
            "cover_meta",
            json!(t),
        ));
        out.push(element(&resolve(&v, cx)?, cx)?);
    }
    let content = [
        x,
        y + (0.45 * IN as f64) as Emu,
        if section { size.cx - x - margin } else { full },
        (size.cy - y - IN).max(IN / 2),
    ];
    Ok(Furniture {
        elements: out,
        content,
    })
}

/// The palette name (or value) of a cover's or section's background.
pub fn cover_background_name(look: &Look, section: bool) -> Value {
    let key = if section {
        "section_background"
    } else {
        "background"
    };
    look.cover.get(key).cloned().unwrap_or(json!("dark"))
}

/// The cover's background (palette names allowed), if any.
pub fn cover_background(look: &Look, cx: &Ctx, section: bool) -> Value {
    resolve_background(&cover_background_name(look, section), cx)
}

/// A string or number setting as text.
fn text_of(o: &Map<String, Value>, key: &str) -> Result<Option<String>> {
    Ok(match o.get(key) {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => Some(s.clone()),
        Some(Value::Number(n)) => Some(n.to_string()),
        Some(_) => bail!("`{key}` must be text"),
    })
}

/// The footer of a slide: a hairline across the content width or a bar
/// along the bottom, logo and the slide's `footer_note` at the left, the
/// footer's text and the page number (a slide number field) at the right,
/// or a small square mark in the corner.
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
            let color = bo.get("color").cloned().unwrap_or(json!("accent"));
            out.push(rect("Footer bar", 0, size.cy - bh, size.cx, bh, color, cx)?);
        }
    }
    match f.get("rule") {
        None | Some(Value::Null) | Some(Value::Bool(false)) => {}
        Some(r) => {
            let ro = match r {
                Value::Object(_) => object(r, "footer `rule`", &["color", "width"])?.clone(),
                Value::Bool(true) => Map::new(),
                _ => bail!("footer `rule` must be {{color, width}} or false"),
            };
            let ry = row_y - (0.07 * IN as f64) as Emu;
            let v = json!({"type": "line", "name": "Footer rule", "x1": e(margin), "y1": e(ry),
                "x2": e(size.cx - margin), "y2": e(ry),
                "color": ro.get("color").cloned().unwrap_or(json!("line")),
                "width": ro.get("width").cloned().unwrap_or(json!("0.75pt"))});
            out.push(element(&resolve(&v, cx)?, cx)?);
        }
    }
    let mut right = size.cx - margin;
    match f.get("mark") {
        None | Some(Value::Null) | Some(Value::Bool(false)) => {}
        Some(m) => {
            let mo = match m {
                Value::Object(_) => object(m, "footer `mark`", &["color", "size"])?.clone(),
                Value::Bool(true) => Map::new(),
                _ => bail!("footer `mark` must be {{color, size}} or false"),
            };
            let s = len_or(&mo, "size", cx, (0.14 * IN as f64) as Emu)?;
            let color = mo.get("color").cloned().unwrap_or(json!("accent"));
            out.push(rect(
                "Footer mark",
                right - s,
                row_y + (row_h - s) / 2,
                s,
                s,
                color,
                cx,
            )?);
            right -= s + (0.2 * IN as f64) as Emu;
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
    if opt_bool(f, "page_numbers")?.unwrap_or(true) {
        let num_w = (0.45 * IN as f64) as Emu;
        let mut v = text_box(
            "Slide number",
            right - num_w,
            row_y,
            num_w,
            row_h,
            "page_number",
            json!([{"runs": [page_run(cx, "page_number")]}]),
        );
        v["anchor"] = json!("middle");
        v["align"] = json!("right");
        v["wrap"] = json!(false);
        out.push(element(&resolve(&v, cx)?, cx)?);
        right -= num_w + (0.1 * IN as f64) as Emu;
    }
    let in_header = look
        .header
        .as_ref()
        .is_some_and(|h| h.get("text") == Some(&Value::Bool(true)));
    if let Some(t) = opt_str(f, "text")?.filter(|_| !in_header) {
        let chars = t.chars().count() as f64;
        let est = (chars * look.size_of(&style, 10.0) * 0.55 * PT as f64) as Emu + IN / 4;
        let w = est.min(right - x - IN);
        if w > IN / 2 {
            let mut v = text_box("Footer", right - w, row_y, w, row_h, &style, json!(t));
            v["anchor"] = json!("middle");
            v["align"] = json!("right");
            v["wrap"] = json!(false);
            out.push(element(&resolve(&v, cx)?, cx)?);
            right -= w;
        }
    }
    let notes_on = opt_bool(f, "note_field")?.unwrap_or(true);
    if let Some(n) = get_str(so, "footer_note") {
        if !notes_on {
            bail!("the deck's footer has `note_field: false`");
        }
        let end = right - IN / 4;
        if end - x > IN / 2 {
            let mut v = text_box(
                "Footer note",
                x,
                row_y,
                end - x,
                row_h,
                &note_style,
                json!(n),
            );
            v["anchor"] = json!("middle");
            out.push(element(&resolve(&v, cx)?, cx)?);
        }
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
    let rule = json!({"description": "A rule above the eyebrow, or beside it (position left): {color, w (a length or \"full\"), h, position, gap}, or false.", "anyOf": [
        {"type": "boolean"},
        {"type": "object", "additionalProperties": false, "properties": {"color": r("color"),
            "w": {"anyOf": [{"type": "string", "const": "full"}, r("length")]}, "h": r("length"),
            "position": {"type": "string", "enum": ["above", "left"]},
            "gap": {"description": "Space under a rule above the eyebrow.", "allOf": [r("length")]}}}]});
    let small = |what: &str| {
        json!({"description": what, "anyOf": [{"type": "boolean"},
        {"type": "object", "additionalProperties": false, "properties": {"color": r("color"), "width": r("length"), "size": r("length")}}]})
    };
    let palette: Map<String, Value> = PALETTE
        .iter()
        .map(|n| ((*n).to_owned(), json!({"type": "string"})))
        .collect();
    json!({
        "preset": {"type": "string", "enum": PRESETS, "description": "A built-in look: editorial (warm paper, Instrument Serif titles and numbers, hairline rules, no boxes), swiss (white, Inter bold titles under a thick rule, mono labels, solid square panels, one strong blue) or mono (swiss in black, white and one red). Sets the theme colours and fonts, palette, text styles, card style, header, footer, covers and the slide background; the deck's own settings go over it (its theme accent recolours the preset's tints)."},
        "card_style": {"type": "string", "enum": ["rule", "fill"], "description": "How cards, stats, comparisons, callouts and clean tables are drawn: rule (no boxes, a rule on top, numbers in the heading font) or fill (square panels, the highlighted one in the accent colour). Default from the preset; fill without one."},
        "palette": {"type": "object", "additionalProperties": false, "properties": palette,
            "description": "Named colours used by styles and components (and allowed wherever a colour is): #RRGGBB or a theme colour. Default from the theme: accent / primary accent1, highlight accent2, ink tx1, dark tx2, panel / paper bg2, background / white / on_accent / on_dark bg1, muted, line / divider, bar, accent_soft / tint, on_dark_accent mixed from them; good / success, bad / danger fixed."},
        "styles": {"type": "object", "additionalProperties": r("text_style"),
            "description": "Named text styles (used with `style`): eyebrow, page_number, title, subtitle, body, small, note, label, card_title, stat, number, numeral, badge, callout, table, table_header, footer, cover_brand, cover_eyebrow, cover_title, cover_subtitle, cover_meta, cover_mark, cover_mark_label, section_eyebrow, section_title, section_subtitle, and any of your own. Keys given here go over the preset's."},
        "header": {"description": "Where a slide's eyebrow, title and subtitle go (a preset turns it on; false: none). Slides with a header put `title` in a title placeholder, and components default to the area under it.",
            "anyOf": [{"type": "boolean"}, {"type": "object", "additionalProperties": false, "properties": {
                "x": r("length"), "y": {"description": "Top of the header (default 0.55in).", "allOf": [r("length")]}, "w": r("length"),
                "gap": r("length"), "content_gap": {"description": "Space between the header and the content area.", "allOf": [r("length")]},
                "rule": rule.clone(), "align": {"type": "string", "enum": ["left", "center"]},
                "page_number": {"type": "boolean", "description": "The page number (02) before the eyebrow."},
                "separator": {"type": "string", "description": "Between the page number and the eyebrow (default \" / \")."},
                "text": {"description": "Text at the right of the eyebrow row; true: the footer's text (then not in the footer).", "type": ["string", "boolean"]}}}]},
        "footer": {"description": "Each slide's footer (cover and section slides have none unless `footer: true`).",
            "anyOf": [{"type": "boolean"}, {"type": "object", "additionalProperties": false, "properties": {
                "text": {"type": "string", "description": "e.g. \"Prepared for …\"."},
                "page_numbers": {"type": "boolean", "description": "A slide number field (02) at the right (default true)."},
                "logo": {"type": "string", "description": "A PNG or JPEG at the left."},
                "logo_h": r("length"),
                "bar": {"description": "A bar along the bottom edge: {color, height}, or false.", "anyOf": [{"type": "boolean"},
                    {"type": "object", "additionalProperties": false, "properties": {"color": r("color"), "height": r("length")}}]},
                "note_field": {"type": "boolean", "description": "Show slides' `footer_note` (default true)."},
                "rule": small("A hairline across the content width above the footer: {color, width}, or false."),
                "mark": small("A small square at the bottom right: {color, size}, or false."),
                "y": r("length"), "style": {"type": "string"}, "note_style": {"type": "string"}}}]},
        "cover": {"type": "object", "additionalProperties": false, "description": "Cover and section slides (`kind`).", "properties": {
            "background": r("background"), "x": r("length"),
            "y": {"description": "Top of the cover's text (default: centred).", "allOf": [r("length")]},
            "w": r("length"), "rule": rule,
            "logo": {"type": "string", "description": "A picture at the top left."}, "logo_h": r("length"), "align": {"type": "string", "enum": ["left", "center"]},
            "motif": {"type": "string", "enum": ["none", "block", "grid"], "description": "block: an accent block down the right carrying the slide's `mark`; grid: a 4×4 grid of squares."},
            "block_w": r("length"),
            "brand": {"type": "boolean", "description": "The footer's text at the top left (when there is no logo)."},
            "section_background": r("background"),
            "section_y": {"description": "Top of a section slide's text (default: centred).", "allOf": [r("length")]}}},
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
        "kind": {"type": "string", "enum": ["content", "cover", "section"], "description": "cover: a big title with the cover's motif; section: a dark slide with a big title, components under it (see the deck's `cover`)."},
        "eyebrow": {"type": "string", "description": "The small label above the title (needs a header or preset)."},
        "subtitle": {"type": "string"},
        "footer": {"type": "boolean", "description": "false hides the deck's footer on this slide."},
        "footer_note": {"type": "string", "description": "Text in the footer, e.g. a source."},
        "meta": {"type": "string", "description": "A cover's small line at the bottom, e.g. who presents."},
        "mark": {"type": ["string", "number"], "description": "A cover's big number in the accent block (cover motif block)."},
        "mark_label": {"type": "string", "description": "A small line at the top of the accent block."},
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
        let deck = json!({"preset": "swiss", "styles": {"eyebrow": {"size": 12}}, "palette": {"muted": "#123456"}});
        let look = Look::build(&p, deck.as_object().unwrap(), &Ctx::default()).unwrap();
        assert_eq!(look.size_of("eyebrow", 0.0), 12.0);
        assert_eq!(look.styles["eyebrow"]["caps"], true);
        assert_eq!(look.card_style, CardStyle::Fill);
        // A deck's own accent: the tints are mixed from it, not the preset's.
        let ed = json!({"preset": "editorial"});
        let ed = Look::build(&p, ed.as_object().unwrap(), &Ctx::default()).unwrap();
        assert_eq!(ed.card_style, CardStyle::Rule);
        assert_eq!(ed.rgb("accent_soft").hex(), "F2DED0");
        assert!(ed.is_dark("dark") && !ed.is_dark("background"));
        assert_eq!(ed.on_dark("dark").rgb("ink"), ed.rgb("on_dark"));
        let own = json!({"preset": "editorial", "theme": {"colors": {"accent1": "#0055AA"}}});
        let own = Look::build(&p, own.as_object().unwrap(), &Ctx::default()).unwrap();
        assert_ne!(own.rgb("accent_soft").hex(), "F2DED0");
        assert!(preset("consulting-navy").is_err());
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
