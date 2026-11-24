//! Slides authored from JSON: the element schema shared by the MCP slides
//! tools, `kantor slides edit` and `kantor slides build` (one schema, see
//! docs/slides-json.md and docs/slides-deck.schema.json), read into
//! [`slides_engine::author`] specs, and the slide tools that edit a
//! [`Presentation`] (run in the app, or headless by the CLI).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context as _, Result, anyhow, bail};
use serde_json::{Map, Value, json};
use slides_engine::author::*;
use slides_engine::{
    Align, Anchor, Emu, Focus, Presentation, Rgb, SlideSize, Spacing, Xfrm, image_size,
};

use super::{MAX_TEXT, check_len};

/// Elements one call (or one slide) may add.
pub const MAX_ELEMENTS: usize = 1000;
/// Slides one deck may have.
pub const MAX_SLIDES: usize = 2000;
/// Largest picture file read (bytes).
const MAX_PICTURE: u64 = 100 * 1024 * 1024;

/// What bare numbers in lengths mean.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Unit {
    #[default]
    Emu,
    Pt,
    In,
    Cm,
    Mm,
    Px,
}

impl Unit {
    fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "emu" => Self::Emu,
            "pt" => Self::Pt,
            "in" => Self::In,
            "cm" => Self::Cm,
            "mm" => Self::Mm,
            "px" => Self::Px,
            _ => return None,
        })
    }

    fn emu_per(self) -> f64 {
        match self {
            Self::Emu => 1.0,
            Self::Pt => 12_700.0,
            Self::In => 914_400.0,
            Self::Cm => 360_000.0,
            Self::Mm => 36_000.0,
            Self::Px => 9_525.0,
        }
    }
}

/// Where values are read: the length unit, the folder relative picture
/// paths start from (none: they must be absolute), the slide size and
/// the deck's default text formatting.
#[derive(Clone, Debug, Default)]
pub struct Ctx {
    pub units: Unit,
    pub base: Option<PathBuf>,
    pub size: Option<SlideSize>,
    pub style: RunStyle,
    /// The deck's styles, palette and furniture ([`super::slides_style`]).
    pub look: Option<Arc<super::slides_style::Look>>,
    /// Where components go without a box: the slide's content area.
    pub area: Option<[Emu; 4]>,
    /// The slide's number (shown by slide number fields until updated).
    pub slide_no: Option<usize>,
}

impl Ctx {
    /// For one tool call: `units` from its arguments.
    pub fn for_tool(p: &Presentation, args: &Value, base: Option<&Path>) -> Result<Self> {
        Ok(Self {
            units: units_arg(args)?,
            base: base.map(Path::to_path_buf),
            size: Some(p.size),
            look: Some(Arc::new(super::slides_style::Look::plain(p))),
            ..Self::default()
        })
    }
}

fn units_arg(args: &Value) -> Result<Unit> {
    match args.get("units") {
        None | Some(Value::Null) => Ok(Unit::Emu),
        Some(Value::String(s)) => Unit::parse(s)
            .ok_or_else(|| anyhow!("`units` must be emu, pt, in, cm, mm or px, not \"{s}\"")),
        Some(_) => bail!("`units` must be a string"),
    }
}

// ── reading values ─────────────────────────────────────────────────────────

/// `v` as an object whose keys are all in `allowed`.
pub(super) fn object<'a>(
    v: &'a Value,
    what: &str,
    allowed: &[&str],
) -> Result<&'a Map<String, Value>> {
    let Some(obj) = v.as_object() else {
        bail!("{what} must be an object");
    };
    for k in obj.keys() {
        if !allowed.contains(&k.as_str()) {
            bail!(
                "{what} has an unknown key \"{k}\" (allowed: {})",
                allowed.join(", ")
            );
        }
    }
    Ok(obj)
}

fn get<'a>(obj: &'a Map<String, Value>, key: &str) -> Option<&'a Value> {
    obj.get(key).filter(|v| !v.is_null())
}

pub(super) fn opt_str<'a>(obj: &'a Map<String, Value>, key: &str) -> Result<Option<&'a str>> {
    match get(obj, key) {
        None => Ok(None),
        Some(Value::String(s)) => Ok(Some(s)),
        Some(_) => bail!("`{key}` must be a string"),
    }
}

pub(super) fn opt_bool(obj: &Map<String, Value>, key: &str) -> Result<Option<bool>> {
    match get(obj, key) {
        None => Ok(None),
        Some(Value::Bool(b)) => Ok(Some(*b)),
        Some(_) => bail!("`{key}` must be true or false"),
    }
}

pub(super) fn opt_num(obj: &Map<String, Value>, key: &str) -> Result<Option<f64>> {
    match get(obj, key) {
        None => Ok(None),
        Some(v) => v
            .as_f64()
            .filter(|n| n.is_finite())
            .map(Some)
            .ok_or_else(|| anyhow!("`{key}` must be a number")),
    }
}

/// A number with an optional unit suffix (`"1.5in"`, `"12pt"`, `"40%"`).
fn split_unit(s: &str) -> Option<(f64, &str)> {
    let s = s.trim();
    let at = s
        .find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-' || c == '+'))
        .unwrap_or(s.len());
    let n: f64 = s[..at].trim().parse().ok()?;
    n.is_finite().then_some((n, s[at..].trim()))
}

/// A length: a number in the context's unit, or a string with a unit
/// (`emu`, `pt`, `in`, `cm`, `mm`, `px`).
pub(super) fn length(v: &Value, cx: &Ctx, key: &str) -> Result<Emu> {
    let emu = match v {
        Value::Number(n) => n.as_f64().unwrap_or(f64::NAN) * cx.units.emu_per(),
        Value::String(s) => {
            let (n, unit) = split_unit(s)
                .ok_or_else(|| anyhow!("`{key}` \"{s}\" is not a length (e.g. \"1.5in\")"))?;
            let unit = if unit.is_empty() {
                cx.units
            } else {
                Unit::parse(unit).ok_or_else(|| {
                    anyhow!("`{key}` \"{s}\": the unit must be emu, pt, in, cm, mm or px")
                })?
            };
            n * unit.emu_per()
        }
        _ => bail!("`{key}` must be a number or a string such as \"2.5cm\""),
    };
    if !emu.is_finite() || emu.abs() > 1.0e11 {
        bail!("`{key}` is out of range");
    }
    Ok(emu.round() as Emu)
}

pub(super) fn opt_len(obj: &Map<String, Value>, key: &str, cx: &Ctx) -> Result<Option<Emu>> {
    get(obj, key).map(|v| length(v, cx, key)).transpose()
}

/// Four lengths (left, top, right, bottom) from one length or an array.
fn four(v: &Value, cx: &Ctx, key: &str) -> Result<[Emu; 4]> {
    match v {
        Value::Array(a) if a.len() == 4 => Ok([
            length(&a[0], cx, key)?,
            length(&a[1], cx, key)?,
            length(&a[2], cx, key)?,
            length(&a[3], cx, key)?,
        ]),
        Value::Array(_) => bail!("`{key}` must be one length or four [left, top, right, bottom]"),
        v => {
            let n = length(v, cx, key)?;
            Ok([n; 4])
        }
    }
}

/// A fraction 0–1 (or `"25%"`).
fn fraction(v: &Value, key: &str) -> Result<f64> {
    let f = match v {
        Value::Number(n) => n.as_f64().unwrap_or(f64::NAN),
        Value::String(s) => match split_unit(s) {
            Some((n, "%")) => n / 100.0,
            Some((n, "")) => n,
            _ => bail!("`{key}` must be a fraction (0.25) or a percentage (\"25%\")"),
        },
        _ => bail!("`{key}` must be a number"),
    };
    if !(0.0..=1.0).contains(&f) {
        bail!("`{key}` must be between 0 and 1");
    }
    Ok(f)
}

/// A font size: points (`18`, `"18pt"`) → hundredths of a point.
fn font_size(v: &Value) -> Result<u32> {
    let pt = match v {
        Value::Number(n) => n.as_f64().unwrap_or(f64::NAN),
        Value::String(s) => match split_unit(s) {
            Some((n, "" | "pt")) => n,
            _ => bail!("`size` must be in points, e.g. 18"),
        },
        _ => bail!("`size` must be a number of points"),
    };
    if !(1.0..=4000.0).contains(&pt) {
        bail!("`size` must be between 1 and 4000 points");
    }
    Ok((pt * 100.0).round() as u32)
}

/// Line spacing: a multiple of single spacing (`1.2`) or points (`"20pt"`).
fn line_spacing(v: &Value) -> Result<Spacing> {
    match v {
        Value::Number(n) => {
            let m = n.as_f64().unwrap_or(f64::NAN);
            if !(0.1..=10.0).contains(&m) {
                bail!("`line_spacing` must be between 0.1 and 10 lines");
            }
            Ok(Spacing::Pct((m * 100_000.0).round() as i32))
        }
        Value::String(s) => match split_unit(s) {
            Some((n, "pt")) if (0.0..=1584.0).contains(&n) => {
                Ok(Spacing::Pts((n * 100.0).round() as i32))
            }
            Some((n, "")) => line_spacing(&json!(n)),
            _ => bail!("`line_spacing` must be lines (1.2) or points (\"20pt\")"),
        },
        _ => bail!("`line_spacing` must be a number or a string"),
    }
}

/// Space before/after a paragraph: points (`6`, `"6pt"`).
fn para_space(v: &Value, key: &str) -> Result<Spacing> {
    let pt = match v {
        Value::Number(n) => n.as_f64().unwrap_or(f64::NAN),
        Value::String(s) => match split_unit(s) {
            Some((n, "" | "pt")) => n,
            _ => bail!("`{key}` must be in points"),
        },
        _ => bail!("`{key}` must be a number of points"),
    };
    if !(0.0..=1584.0).contains(&pt) {
        bail!("`{key}` must be between 0 and 1584 points");
    }
    Ok(Spacing::Pts((pt * 100.0).round() as i32))
}

/// A colour: `#RRGGBB` (`#RGB`, `#RRGGBBAA`), a theme colour name, or
/// `{"color": ..., "opacity": 0.5}`.
pub fn color(v: &Value, key: &str) -> Result<Color> {
    match v {
        Value::String(s) => {
            let s = s.trim();
            if let Some(name) = SCHEME_COLORS.iter().find(|n| n.eq_ignore_ascii_case(s)) {
                return Ok(Color::scheme(name));
            }
            let hex = s.strip_prefix('#').unwrap_or(s);
            let ok = hex.chars().all(|c| c.is_ascii_hexdigit());
            let (rgb, alpha) = match hex.len() {
                3 if ok => {
                    let d: String = hex.chars().flat_map(|c| [c, c]).collect();
                    (d, None)
                }
                6 if ok => (hex.to_owned(), None),
                8 if ok => {
                    let a = u8::from_str_radix(&hex[6..], 16).unwrap_or(255);
                    (hex[..6].to_owned(), Some(a as u32 * 100_000 / 255))
                }
                _ => bail!(
                    "`{key}` \"{s}\" is not a colour: use #RRGGBB or a theme colour ({})",
                    SCHEME_COLORS.join(", ")
                ),
            };
            Ok(Color {
                base: slides_engine::author::ColorBase::Rgb(
                    Rgb::parse(&rgb.to_ascii_uppercase()).context("colour")?,
                ),
                alpha,
            })
        }
        Value::Object(_) => {
            let obj = object(v, &format!("`{key}`"), &["color", "opacity"])?;
            let mut c = color(
                get(obj, "color").context("a colour object needs `color`")?,
                key,
            )?;
            if let Some(o) = get(obj, "opacity") {
                c.alpha = Some((fraction(o, "opacity")? * 100_000.0).round() as u32);
            }
            Ok(c)
        }
        _ => bail!("`{key}` must be a colour such as \"#1F3864\" or \"accent1\""),
    }
}

fn is_none(v: &Value) -> bool {
    v.as_str().is_some_and(|s| s.eq_ignore_ascii_case("none"))
}

/// A fill: `"none"`, a colour, or `{"gradient": {...}}`.
pub fn fill(v: &Value, key: &str) -> Result<FillSpec> {
    if is_none(v) {
        return Ok(FillSpec::None);
    }
    if let Some(g) = v.get("gradient") {
        object(v, &format!("`{key}`"), &["gradient"])?;
        let g = object(g, "`gradient`", &["angle", "type", "stops"])?;
        let kind = match opt_str(g, "type")?.unwrap_or("linear") {
            "linear" => GradientKind::Linear(opt_num(g, "angle")?.unwrap_or(90.0)),
            "radial" => GradientKind::Radial,
            other => bail!("gradient `type` must be linear or radial, not \"{other}\""),
        };
        let stops = get(g, "stops")
            .and_then(Value::as_array)
            .context("a gradient needs `stops`: two or more colours")?;
        if stops.len() < 2 || stops.len() > 10 {
            bail!("a gradient needs 2 to 10 `stops`");
        }
        let n = stops.len() - 1;
        let stops = stops
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let even = (i as f64 / n as f64 * 100_000.0).round() as u32;
                match s {
                    Value::Object(o) if o.contains_key("position") => {
                        let o = object(s, "a gradient stop", &["color", "opacity", "position"])?;
                        let pos = fraction(&o["position"], "position")?;
                        let mut c =
                            json!({"color": o.get("color").cloned().unwrap_or(Value::Null)});
                        if let Some(op) = o.get("opacity") {
                            c["opacity"] = op.clone();
                        }
                        Ok(((pos * 100_000.0).round() as u32, color(&c, "stops")?))
                    }
                    s => Ok((even, color(s, "stops")?)),
                }
            })
            .collect::<Result<Vec<_>>>()?;
        return Ok(FillSpec::Gradient { stops, kind });
    }
    Ok(FillSpec::Solid(color(v, key)?))
}

fn arrow(v: &Value) -> Result<ArrowSpec> {
    let size = |s: Option<&str>, key: &str| -> Result<String> {
        Ok(match s.unwrap_or("medium") {
            "small" | "sm" => "sm",
            "medium" | "med" => "med",
            "large" | "lg" => "lg",
            other => bail!("arrow `{key}` must be small, medium or large, not \"{other}\""),
        }
        .to_owned())
    };
    let (kind, w, l) = match v {
        Value::String(s) => (s.as_str(), None, None),
        Value::Object(_) => {
            let o = object(v, "an arrowhead", &["type", "width", "length"])?;
            (
                opt_str(o, "type")?.unwrap_or("triangle"),
                opt_str(o, "width")?,
                opt_str(o, "length")?,
            )
        }
        _ => bail!("an arrowhead must be a type (\"triangle\") or {{type, width, length}}"),
    };
    if !ARROW_KINDS.contains(&kind) {
        bail!(
            "arrowhead type \"{kind}\" is not one of {}",
            ARROW_KINDS.join(", ")
        );
    }
    Ok(ArrowSpec {
        kind: kind.to_owned(),
        width: size(w, "width")?,
        length: size(l, "length")?,
    })
}

const LINE_KEYS: &[&str] = &["color", "opacity", "width", "dash", "head", "tail", "cap"];

/// An outline or line: `"none"`, a colour, or `{color, width, dash,
/// head, tail, cap}`.
pub fn line(v: &Value, cx: &Ctx, key: &str) -> Result<LineSpec> {
    if is_none(v) {
        return Ok(LineSpec::none());
    }
    if v.is_string() {
        return Ok(LineSpec {
            fill: Some(FillSpec::Solid(color(v, key)?)),
            ..LineSpec::default()
        });
    }
    let o = object(v, &format!("`{key}`"), LINE_KEYS)?;
    let mut out = LineSpec::default();
    if let Some(c) = get(o, "color") {
        out.fill = Some(if is_none(c) {
            FillSpec::None
        } else {
            let mut c = color(c, "color")?;
            if let Some(op) = get(o, "opacity") {
                c.alpha = Some((fraction(op, "opacity")? * 100_000.0).round() as u32);
            }
            FillSpec::Solid(c)
        });
    }
    out.width = opt_len(o, "width", cx)?;
    if let Some(d) = opt_str(o, "dash")? {
        if !DASHES.contains(&d) {
            bail!("`dash` \"{d}\" is not one of {}", DASHES.join(", "));
        }
        out.dash = Some(d.to_owned());
    }
    out.head = get(o, "head").map(arrow).transpose()?;
    out.tail = get(o, "tail").map(arrow).transpose()?;
    out.round_cap = match opt_str(o, "cap")? {
        None | Some("flat") => false,
        Some("round") => true,
        Some(other) => bail!("`cap` must be flat or round, not \"{other}\""),
    };
    Ok(out)
}

const STYLE_KEYS: &[&str] = &[
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

/// Character formatting from the style keys of `o`.
fn run_style(o: &Map<String, Value>) -> Result<RunStyle> {
    Ok(RunStyle {
        font: opt_str(o, "font")?.map(str::to_owned),
        size: get(o, "size").map(font_size).transpose()?,
        bold: opt_bool(o, "bold")?,
        italic: opt_bool(o, "italic")?,
        underline: opt_bool(o, "underline")?,
        strike: opt_bool(o, "strike")?,
        color: get(o, "color").map(|c| color(c, "color")).transpose()?,
        caps: opt_bool(o, "caps")?,
        spacing: get(o, "letter_spacing").map(letter_spacing).transpose()?,
    })
}

/// Character spacing: points (`1`, `"1.5pt"`) → hundredths of a point.
fn letter_spacing(v: &Value) -> Result<i32> {
    let pt = match v {
        Value::Number(n) => n.as_f64().unwrap_or(f64::NAN),
        Value::String(s) => match split_unit(s) {
            Some((n, "" | "pt")) => n,
            _ => bail!("`letter_spacing` must be in points, e.g. 1"),
        },
        _ => bail!("`letter_spacing` must be a number of points"),
    };
    if !(-100.0..=400.0).contains(&pt) {
        bail!("`letter_spacing` must be between -100 and 400 points");
    }
    Ok((pt * 100.0).round() as i32)
}

/// Text whose formatting says all capitals, written in capitals: Kantor
/// draws text as written, PowerPoint also sees `cap="all"`.
fn caps_text(paras: &mut [ParaSpec], base: &RunStyle) {
    for p in paras {
        for r in &mut p.runs {
            if r.field.is_none() && base.over(&r.style).caps == Some(true) {
                r.text = r.text.to_uppercase();
            }
        }
    }
}

/// A run's field: `slide_number`.
fn field(v: &str) -> Result<&'static str> {
    match v {
        "slide_number" => Ok("slidenum"),
        other => bail!("run `field` must be slide_number, not \"{other}\""),
    }
}

fn align(v: &str) -> Result<Align> {
    Ok(match v {
        "left" => Align::Left,
        "center" | "centre" => Align::Center,
        "right" => Align::Right,
        "justify" => Align::Justify,
        other => bail!("`align` must be left, center, right or justify, not \"{other}\""),
    })
}

fn anchor(v: &str) -> Result<Anchor> {
    Ok(match v {
        "top" => Anchor::Top,
        "middle" | "center" => Anchor::Middle,
        "bottom" => Anchor::Bottom,
        other => bail!("`anchor` must be top, middle or bottom, not \"{other}\""),
    })
}

/// Hanging indent of a bulleted paragraph (as PowerPoint's bullets).
const BULLET_INDENT: Emu = 285_750;

fn bullet(v: &Value) -> Result<BulletSpec> {
    Ok(match v {
        Value::Bool(false) => BulletSpec::None,
        Value::Bool(true) => BulletSpec::Char("•".into()),
        Value::String(s) => match s.as_str() {
            "none" => BulletSpec::None,
            "number" => BulletSpec::Number("arabicPeriod".into()),
            s if s.chars().count() == 1 => BulletSpec::Char(s.into()),
            s if s.len() > 4 && s.chars().all(|c| c.is_ascii_alphabetic()) => {
                BulletSpec::Number(s.into())
            }
            _ => bail!(
                "`bullet` must be true, false, \"number\", one character such as \"–\", or a numbering scheme such as \"alphaLcParenR\""
            ),
        },
        _ => bail!("`bullet` must be true, false or a string"),
    })
}

/// Paragraph settings that can be given for a whole text or per paragraph.
#[derive(Clone, Debug, Default)]
struct ParaDefaults {
    align: Option<Align>,
    line_spacing: Option<Spacing>,
    space_before: Option<Spacing>,
    space_after: Option<Spacing>,
    bullet: Option<BulletSpec>,
    level: u8,
}

impl ParaDefaults {
    fn read(o: &Map<String, Value>, base: &ParaDefaults) -> Result<Self> {
        let level = match get(o, "level") {
            None => base.level,
            Some(v) => match v.as_u64() {
                Some(n) if n <= 8 => n as u8,
                _ => bail!("`level` must be 0 to 8"),
            },
        };
        Ok(Self {
            align: opt_str(o, "align")?.map(align).transpose()?.or(base.align),
            line_spacing: get(o, "line_spacing")
                .map(line_spacing)
                .transpose()?
                .or(base.line_spacing),
            space_before: get(o, "space_before")
                .map(|v| para_space(v, "space_before"))
                .transpose()?
                .or(base.space_before),
            space_after: get(o, "space_after")
                .map(|v| para_space(v, "space_after"))
                .transpose()?
                .or(base.space_after),
            bullet: get(o, "bullet")
                .map(bullet)
                .transpose()?
                .or_else(|| base.bullet.clone()),
            level,
        })
    }

    fn para(&self, runs: Vec<RunSpec>, end_style: RunStyle) -> ParaSpec {
        let bulleted = matches!(
            self.bullet,
            Some(BulletSpec::Char(_) | BulletSpec::Number(_))
        );
        ParaSpec {
            runs,
            align: self.align,
            level: self.level,
            line_spacing: self.line_spacing,
            space_before: self.space_before,
            space_after: self.space_after,
            bullet: self.bullet.clone(),
            margin_left: bulleted.then_some(BULLET_INDENT * (self.level as Emu + 1)),
            indent: bulleted.then_some(-BULLET_INDENT),
            end_style,
        }
    }
}

const PARA_KEYS: &[&str] = &[
    "text",
    "runs",
    "align",
    "level",
    "bullet",
    "line_spacing",
    "space_before",
    "space_after",
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

const RUN_KEYS: &[&str] = &[
    "text",
    "field",
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

/// Text with `\r\n` and `\r` as `\n`, no other control characters but tabs.
fn clean(text: &str) -> Result<String> {
    check_len("text", text)?;
    Ok(text
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .chars()
        .filter(|&c| c == '\t' || c == '\n' || !c.is_control())
        .collect())
}

/// The paragraphs of `v`: a string (a new line starts a paragraph) or an
/// array of paragraphs (strings or objects with `text` or `runs`).
fn paragraphs(v: &Value, defaults: &ParaDefaults, cx: &Ctx) -> Result<Vec<ParaSpec>> {
    let plain = |text: &str, d: &ParaDefaults, style: RunStyle| -> Result<Vec<ParaSpec>> {
        Ok(clean(text)?
            .split('\n')
            .map(|l| {
                d.para(
                    vec![RunSpec {
                        text: l.to_owned(),
                        style: style.clone(),
                        field: None,
                    }],
                    style.clone(),
                )
            })
            .collect())
    };
    match v {
        Value::String(s) => plain(s, defaults, RunStyle::default()),
        Value::Number(n) => plain(&n.to_string(), defaults, RunStyle::default()),
        Value::Array(items) => {
            if items.len() > 5000 {
                bail!("more than 5000 paragraphs in one text");
            }
            let mut out = Vec::new();
            let mut total = 0usize;
            for item in items {
                match item {
                    Value::String(s) => {
                        // One paragraph each; a new line inside is a line break.
                        out.push(defaults.para(
                            vec![RunSpec {
                                text: clean(s)?,
                                style: RunStyle::default(),
                                field: None,
                            }],
                            RunStyle::default(),
                        ));
                    }
                    Value::Object(_) => {
                        let o = object(item, "a paragraph", PARA_KEYS)?;
                        let d = ParaDefaults::read(o, defaults)?;
                        let style = run_style(o)?;
                        let runs = match (get(o, "text"), get(o, "runs")) {
                            (Some(_), Some(_)) => {
                                bail!("a paragraph has `text` or `runs`, not both")
                            }
                            (Some(Value::String(t)), None) => vec![RunSpec {
                                text: clean(t)?,
                                style: style.clone(),
                                field: None,
                            }],
                            (Some(_), None) => bail!("a paragraph's `text` must be a string"),
                            (None, Some(Value::Array(runs))) => runs
                                .iter()
                                .map(|r| match r {
                                    Value::String(t) => Ok(RunSpec {
                                        text: clean(t)?,
                                        style: style.clone(),
                                        field: None,
                                    }),
                                    _ => {
                                        let ro = object(r, "a run", RUN_KEYS)?;
                                        let fld = opt_str(ro, "field")?.map(field).transpose()?;
                                        let text = match (opt_str(ro, "text")?, fld) {
                                            (Some(t), _) => clean(t)?,
                                            (None, Some(_)) => cx
                                                .slide_no
                                                .map_or_else(|| "#".to_owned(), |n| n.to_string()),
                                            (None, None) => bail!("a run needs `text`"),
                                        };
                                        Ok(RunSpec {
                                            text,
                                            style: style.over(&run_style(ro)?),
                                            field: fld.map(str::to_owned),
                                        })
                                    }
                                })
                                .collect::<Result<_>>()?,
                            (None, Some(_)) => bail!("`runs` must be an array"),
                            (None, None) => Vec::new(),
                        };
                        out.push(d.para(runs, style));
                    }
                    _ => bail!("a paragraph must be a string or an object"),
                }
                total += out
                    .last()
                    .map_or(0, |p| p.runs.iter().map(|r| r.text.chars().count()).sum());
                if total > MAX_TEXT * 4 {
                    bail!("the text is longer than {} characters", MAX_TEXT * 4);
                }
            }
            Ok(out)
        }
        _ => bail!("`text` must be a string or an array of paragraphs"),
    }
}

const TEXT_KEYS: &[&str] = &[
    "text",
    "align",
    "anchor",
    "insets",
    "autofit",
    "wrap",
    "vertical",
    "line_spacing",
    "space_before",
    "space_after",
    "bullet",
    "level",
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

/// The text of a shape or cell (`None` without `text`), and its default
/// run formatting.
fn text_body(
    o: &Map<String, Value>,
    cx: &Ctx,
    base: ParaDefaults,
    anchor_default: Option<Anchor>,
) -> Result<(Option<TextSpec>, RunStyle)> {
    let style = cx.style.over(&run_style(o)?);
    let defaults = ParaDefaults::read(o, &base)?;
    let anchor_v = opt_str(o, "anchor")?.map(anchor).transpose()?;
    let text_given = get(o, "text").is_some();
    let paras = match get(o, "text") {
        Some(t) => paragraphs(t, &defaults, cx)?,
        None => vec![defaults.para(Vec::new(), RunStyle::default())],
    };
    let autofit = match opt_str(o, "autofit")? {
        None | Some("none") => Autofit::None,
        Some("shrink") => Autofit::Shrink,
        Some("resize") => Autofit::Resize,
        Some(other) => bail!("`autofit` must be none, shrink or resize, not \"{other}\""),
    };
    let vertical = match opt_str(o, "vertical")? {
        None | Some("horizontal") => None,
        Some("down") => Some("vert".to_owned()),
        Some("up") => Some("vert270".to_owned()),
        Some(other) => bail!("`vertical` must be horizontal, down or up, not \"{other}\""),
    };
    let spec = TextSpec {
        paras,
        anchor: anchor_v.or(anchor_default),
        insets: get(o, "insets")
            .map(|v| four(v, cx, "insets"))
            .transpose()?,
        autofit,
        wrap: opt_bool(o, "wrap")?.unwrap_or(true),
        vertical,
    };
    let any_layout = ["anchor", "insets", "autofit", "wrap", "vertical"]
        .iter()
        .any(|k| get(o, k).is_some());
    Ok(((text_given || any_layout).then_some(spec), style))
}

/// Friendly names for a few presets.
fn preset_name(name: &str) -> &str {
    match name {
        "rectangle" => "rect",
        "rounded" | "rounded_rect" | "roundedRect" => "roundRect",
        "oval" | "circle" => "ellipse",
        other => other,
    }
}

/// The geometry from `geometry`, `radius` and `adjust` (for a `w` × `h` box).
fn geometry(
    o: &Map<String, Value>,
    cx: &Ctx,
    default: &str,
    w: Emu,
    h: Emu,
) -> Result<Option<GeomSpec>> {
    let given = opt_str(o, "geometry")?;
    if given.is_none() && get(o, "radius").is_none() && get(o, "adjust").is_none() {
        return Ok(None);
    }
    let mut name = preset_name(given.unwrap_or(default)).to_owned();
    if given.is_none() && get(o, "radius").is_some() {
        name = "roundRect".into();
    }
    let mut g = GeomSpec::preset(&name);
    if let Some(r) = opt_len(o, "radius", cx)? {
        let adj = match name.as_str() {
            "roundRect" | "round1Rect" => "adj",
            "round2SameRect" | "round2DiagRect" => "adj1",
            _ => bail!(
                "`radius` is for rounded rectangles (roundRect, round1Rect, round2SameRect, round2DiagRect); use `adjust` for {name}"
            ),
        };
        g.adjust.push((adj.into(), round_rect_adj(r, w, h)));
    }
    if let Some(a) = get(o, "adjust") {
        let Some(a) = a.as_object() else {
            bail!("`adjust` must be an object such as {{\"adj\": 25000}}");
        };
        for (k, v) in a {
            let n = v
                .as_f64()
                .filter(|n| n.is_finite() && n.abs() < 1e9)
                .with_context(|| format!("adjust value `{k}` must be a number"))?;
            g.adjust.retain(|(name, _)| name != k);
            g.adjust.push((k.clone(), n.round() as i64));
        }
    }
    g.check()?;
    Ok(Some(g))
}

/// Rotation and flips.
fn turn(o: &Map<String, Value>, x: &mut Xfrm) -> Result<()> {
    if let Some(deg) = opt_num(o, "rotation")? {
        *x = x.with_degrees(deg);
    }
    x.flip_h = opt_bool(o, "flip_h")?.unwrap_or(false);
    x.flip_v = opt_bool(o, "flip_v")?.unwrap_or(false);
    Ok(())
}

fn need_len(o: &Map<String, Value>, key: &str, cx: &Ctx, what: &str) -> Result<Emu> {
    opt_len(o, key, cx)?.with_context(|| format!("{what} needs `{key}`"))
}

const BOX_KEYS: &[&str] = &[
    "type", "name", "x", "y", "w", "h", "rotation", "flip_h", "flip_v",
];

pub(super) fn keys_with(lists: &[&[&'static str]]) -> Vec<&'static str> {
    let mut v: Vec<&str> = lists.iter().flat_map(|l| l.iter().copied()).collect();
    v.sort_unstable();
    v.dedup();
    v
}

/// Read a file as the context resolves it (`~/`, relative to `base`).
fn read_file(path: &str, cx: &Ctx) -> Result<(PathBuf, Vec<u8>)> {
    let text = path.trim();
    let mut p = match text.strip_prefix("~/") {
        Some(rest) => kantor_core::dirs::user_home_dir()
            .context("HOME is not set")?
            .join(rest),
        None => PathBuf::from(text),
    };
    if !p.is_absolute() {
        match &cx.base {
            Some(b) => p = b.join(p),
            None => bail!("give an absolute picture path (got \"{text}\")"),
        }
    }
    let meta = std::fs::metadata(&p).with_context(|| format!("can't read {}", p.display()))?;
    if !meta.is_file() {
        bail!("{} is not a file", p.display());
    }
    if meta.len() > MAX_PICTURE {
        bail!("{} is larger than 100 MB", p.display());
    }
    let bytes = std::fs::read(&p).with_context(|| format!("can't read {}", p.display()))?;
    Ok((p, bytes))
}

/// One element (`type`: shape, text, line, picture, table).
pub fn element(v: &Value, cx: &Ctx) -> Result<Element> {
    let ty = v
        .get("type")
        .and_then(Value::as_str)
        .context("an element needs `type`: shape, text, line, picture, table or icon")?;
    let v = &super::slides_style::resolve(v, cx)?;
    match ty {
        "shape" | "text" => shape_element(v, cx, ty == "text"),
        "line" => line_element(v, cx),
        "picture" => picture_element(v, cx),
        "table" => table_element(v, cx),
        "icon" => super::slides_icons::icon_element(v, cx),
        c if super::slides_components::COMPONENTS.contains(&c) => {
            bail!(
                "\"{c}\" is a component: give it in `elements` (a deck slide or slides_add_elements)"
            )
        }
        other => {
            bail!(
                "element `type` must be shape, text, line, picture, table, icon or a component ({}), not \"{other}\"",
                super::slides_components::COMPONENTS.join(", ")
            )
        }
    }
}

fn shape_element(v: &Value, cx: &Ctx, text_box: bool) -> Result<Element> {
    let allowed = keys_with(&[
        BOX_KEYS,
        TEXT_KEYS,
        &["geometry", "radius", "adjust", "fill", "line"],
    ]);
    let what = if text_box { "a text box" } else { "a shape" };
    let o = object(v, what, &allowed)?;
    let mut xfrm = Xfrm::new(
        opt_len(o, "x", cx)?.unwrap_or(0),
        opt_len(o, "y", cx)?.unwrap_or(0),
        need_len(o, "w", cx, what)?.max(0),
        need_len(o, "h", cx, what)?.max(0),
    );
    turn(o, &mut xfrm)?;
    let geom =
        geometry(o, cx, "rect", xfrm.cx, xfrm.cy)?.unwrap_or_else(|| GeomSpec::preset("rect"));
    let base = ParaDefaults {
        align: (!text_box).then_some(Align::Center),
        ..ParaDefaults::default()
    };
    let (text, style) = text_body(o, cx, base, (!text_box).then_some(Anchor::Middle))?;
    // A shape without text still gets its centred text settings.
    let text = text.or_else(|| {
        (!text_box).then(|| TextSpec {
            paras: vec![ParaSpec {
                align: Some(Align::Center),
                ..ParaSpec::default()
            }],
            anchor: Some(Anchor::Middle),
            wrap: true,
            ..TextSpec::default()
        })
    });
    let mut text = text.or_else(|| {
        Some(TextSpec {
            paras: vec![ParaSpec::default()],
            wrap: true,
            ..TextSpec::default()
        })
    });
    if let Some(t) = &mut text {
        caps_text(&mut t.paras, &style);
    }
    Ok(Element {
        kind: ElementKind::Shape(ShapeSpec {
            geom,
            text_box,
            fill: get(o, "fill").map(|f| fill(f, "fill")).transpose()?,
            line: get(o, "line").map(|l| line(l, cx, "line")).transpose()?,
            text,
            style,
            placeholder: None,
        }),
        name: opt_str(o, "name")?.map(str::to_owned),
        xfrm,
    })
}

fn line_element(v: &Value, cx: &Ctx) -> Result<Element> {
    let o = object(
        v,
        "a line",
        &[
            "type", "name", "x1", "y1", "x2", "y2", "line", "color", "width", "dash", "head",
            "tail", "cap",
        ],
    )?;
    let (x1, y1) = (
        need_len(o, "x1", cx, "a line")?,
        need_len(o, "y1", cx, "a line")?,
    );
    let (x2, y2) = (
        need_len(o, "x2", cx, "a line")?,
        need_len(o, "y2", cx, "a line")?,
    );
    let xfrm = Xfrm {
        flip_h: x2 < x1,
        flip_v: y2 < y1,
        ..Xfrm::new(x1.min(x2), y1.min(y2), (x2 - x1).abs(), (y2 - y1).abs())
    };
    // `line: {...}` or its keys right on the element.
    let mut spec = match get(o, "line") {
        Some(l) => line(l, cx, "line")?,
        None => LineSpec::default(),
    };
    let flat: Map<String, Value> = o
        .iter()
        .filter(|(k, _)| LINE_KEYS.contains(&k.as_str()))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    if !flat.is_empty() {
        if get(o, "line").is_some() {
            bail!(
                "give a line's color, width, dash, head and tail either in `line` or on the element, not both"
            );
        }
        spec = line(&Value::Object(flat), cx, "line")?;
    }
    Ok(Element {
        kind: ElementKind::Line(LineShapeSpec { line: spec }),
        name: opt_str(o, "name")?.map(str::to_owned),
        xfrm,
    })
}

fn picture_element(v: &Value, cx: &Ctx) -> Result<Element> {
    let allowed = keys_with(&[
        BOX_KEYS,
        &[
            "path",
            "crop",
            "fit",
            "keep_aspect",
            "geometry",
            "radius",
            "adjust",
            "line",
            "alt",
        ],
    ]);
    let o = object(v, "a picture", &allowed)?;
    let (path, bytes) = read_file(opt_str(o, "path")?.context("a picture needs `path`")?, cx)?;
    if slides_engine::image_ext(&bytes).is_none() {
        bail!("{} is not a PNG or JPEG picture", path.display());
    }
    let (iw, ih) = image_size(&bytes).unwrap_or((400, 300));
    let (iw, ih) = (iw.max(1) as f64, ih.max(1) as f64);
    let mut crop = [0.0f64; 4];
    if let Some(c) = get(o, "crop") {
        let c = object(c, "`crop`", &["left", "top", "right", "bottom"])?;
        for (i, k) in ["left", "top", "right", "bottom"].iter().enumerate() {
            if let Some(v) = get(c, k) {
                crop[i] = fraction(v, k)?;
            }
        }
        if crop[0] + crop[2] >= 0.999 || crop[1] + crop[3] >= 0.999 {
            bail!("`crop` must leave part of the picture");
        }
    }
    // The aspect of what's left after cropping.
    let aspect =
        |crop: &[f64; 4]| (iw * (1.0 - crop[0] - crop[2])) / (ih * (1.0 - crop[1] - crop[3]));
    let (x, y) = (
        opt_len(o, "x", cx)?.unwrap_or(0),
        opt_len(o, "y", cx)?.unwrap_or(0),
    );
    let (w, h) = (opt_len(o, "w", cx)?, opt_len(o, "h", cx)?);
    let a = aspect(&crop);
    let fit = match (opt_str(o, "fit")?, opt_bool(o, "keep_aspect")?) {
        (Some(f), _) => f,
        (None, Some(true)) => "contain",
        (None, _) => "stretch",
    };
    let mut xfrm = match (w, h) {
        (Some(w), Some(h)) => Xfrm::new(x, y, w.max(1), h.max(1)),
        (Some(w), None) => Xfrm::new(x, y, w.max(1), (w as f64 / a).round() as Emu),
        (None, Some(h)) => Xfrm::new(x, y, (h as f64 * a).round() as Emu, h.max(1)),
        (None, None) => {
            let px = slides_engine::EMU_PER_PX as f64;
            let w = iw * (1.0 - crop[0] - crop[2]) * px;
            Xfrm::new(x, y, w.round() as Emu, (w / a).round() as Emu)
        }
    };
    if w.is_some() && h.is_some() {
        let ba = xfrm.cx as f64 / xfrm.cy as f64;
        match fit {
            "stretch" => {}
            "contain" => {
                if a > ba {
                    let nh = (xfrm.cx as f64 / a).round() as Emu;
                    xfrm.y += (xfrm.cy - nh) / 2;
                    xfrm.cy = nh;
                } else {
                    let nw = (xfrm.cy as f64 * a).round() as Emu;
                    xfrm.x += (xfrm.cx - nw) / 2;
                    xfrm.cx = nw;
                }
            }
            "cover" => {
                let (rw, rh) = (1.0 - crop[0] - crop[2], 1.0 - crop[1] - crop[3]);
                if a > ba {
                    let extra = rw * (1.0 - ba / a);
                    crop[0] += extra / 2.0;
                    crop[2] += extra / 2.0;
                } else {
                    let extra = rh * (1.0 - a / ba);
                    crop[1] += extra / 2.0;
                    crop[3] += extra / 2.0;
                }
            }
            other => bail!("`fit` must be stretch, contain or cover, not \"{other}\""),
        }
    } else if !matches!(fit, "stretch" | "contain" | "cover") {
        bail!("`fit` must be stretch, contain or cover, not \"{fit}\"");
    }
    turn(o, &mut xfrm)?;
    let geom = geometry(o, cx, "rect", xfrm.cx, xfrm.cy)?;
    Ok(Element {
        kind: ElementKind::Picture(PictureSpec {
            bytes: Arc::new(bytes),
            crop: crop.map(|c| (c * 100_000.0).round() as i64),
            geom,
            line: get(o, "line").map(|l| line(l, cx, "line")).transpose()?,
            alt: opt_str(o, "alt")?.map(str::to_owned),
        }),
        name: opt_str(o, "name")?.map(str::to_owned),
        xfrm,
    })
}

/// A table cell's own settings.
const CELL_KEYS: &[&str] = &[
    "text",
    "align",
    "anchor",
    "line_spacing",
    "space_before",
    "space_after",
    "bullet",
    "level",
    "font",
    "size",
    "bold",
    "italic",
    "underline",
    "strike",
    "color",
    "caps",
    "letter_spacing",
    "fill",
    "border",
    "margins",
    "col_span",
    "row_span",
];

fn table_element(v: &Value, cx: &Ctx) -> Result<Element> {
    let allowed = keys_with(&[
        &["type", "name", "x", "y", "w", "h"],
        &[
            "columns",
            "rows",
            "row_height",
            "row_heights",
            "header",
            "banded",
            "header_fill",
            "header_color",
            "fill",
            "band_fill",
            "border",
            "cell_margins",
            "style_id",
            "align",
            "anchor",
            "font",
            "size",
            "bold",
            "italic",
            "underline",
            "color",
            "caps",
            "letter_spacing",
        ],
    ]);
    let o = object(v, "a table", &allowed)?;
    let rows_v = get(o, "rows")
        .and_then(Value::as_array)
        .context("a table needs `rows`: an array of rows, each an array of cells")?;
    if rows_v.is_empty() || rows_v.len() > 1000 {
        bail!("a table needs 1 to 1000 rows");
    }
    let w = opt_len(o, "w", cx)?;
    let columns: Vec<Emu> = match get(o, "columns") {
        Some(Value::Array(a)) => a
            .iter()
            .map(|c| length(c, cx, "columns"))
            .collect::<Result<_>>()?,
        Some(n) if n.is_u64() => {
            let n = n.as_u64().unwrap_or(0) as usize;
            let w = w.context("`columns` as a count needs the table's `w`")?;
            vec![w / n.max(1) as Emu; n]
        }
        Some(_) => bail!("`columns` must be an array of widths or a number of columns"),
        None => {
            let n = rows_v.first().and_then(Value::as_array).map_or(0, Vec::len);
            let w = w.context("a table needs `columns` (widths) or `w`")?;
            vec![w / n.max(1) as Emu; n]
        }
    };
    if columns.is_empty() || columns.len() > 100 {
        bail!("a table needs 1 to 100 columns");
    }
    // Columns given as widths and a `w`: scaled to it.
    let columns = match (w, get(o, "columns").is_some_and(Value::is_array)) {
        (Some(w), true) => {
            let sum: Emu = columns.iter().sum::<Emu>().max(1);
            columns
                .iter()
                .map(|c| (*c as f64 * w as f64 / sum as f64).round() as Emu)
                .collect()
        }
        _ => columns,
    };
    let n_rows = rows_v.len();
    let heights: Vec<Emu> = match (
        get(o, "row_heights"),
        opt_len(o, "row_height", cx)?,
        opt_len(o, "h", cx)?,
    ) {
        (Some(Value::Array(a)), _, _) => {
            if a.len() != n_rows {
                bail!("`row_heights` has {} heights for {n_rows} rows", a.len());
            }
            a.iter()
                .map(|h| length(h, cx, "row_heights"))
                .collect::<Result<_>>()?
        }
        (Some(_), _, _) => bail!("`row_heights` must be an array"),
        (None, Some(h), _) => vec![h; n_rows],
        (None, None, Some(h)) => vec![h / n_rows as Emu; n_rows],
        // PowerPoint's row for one line of 18 pt text.
        (None, None, None) => vec![370_840; n_rows],
    };
    let header = opt_bool(o, "header")?.unwrap_or(true);
    let banded = opt_bool(o, "banded")?.unwrap_or(false);
    let header_fill = match get(o, "header_fill") {
        Some(f) => fill(f, "header_fill")?,
        None => FillSpec::Solid(Color::scheme("accent1")),
    };
    let header_color = match get(o, "header_color") {
        Some(c) => color(c, "header_color")?,
        None => Color::scheme("bg1"),
    };
    let body_fill = get(o, "fill").map(|f| fill(f, "fill")).transpose()?;
    let band_fill = match get(o, "band_fill") {
        Some(f) => fill(f, "band_fill")?,
        None => FillSpec::Solid(Color::rgb(Rgb([0xF2, 0xF2, 0xF2]))),
    };
    let border = match get(o, "border") {
        Some(b) => line(b, cx, "border")?,
        None => LineSpec {
            fill: Some(FillSpec::Solid(Color::rgb(Rgb([0xBF, 0xBF, 0xBF])))),
            width: Some(12_700),
            ..LineSpec::default()
        },
    };
    let margins = get(o, "cell_margins")
        .map(|m| four(m, cx, "cell_margins"))
        .transpose()?;
    let base_defaults = ParaDefaults::read(
        &o.iter()
            .filter(|(k, _)| matches!(k.as_str(), "align"))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect::<Map<String, Value>>(),
        &ParaDefaults::default(),
    )?;
    let table_anchor = opt_str(o, "anchor")?.map(anchor).transpose()?;
    let table_style = cx.style.over(&run_style(
        &o.iter()
            .filter(|(k, _)| STYLE_KEYS.contains(&k.as_str()))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect::<Map<String, Value>>(),
    )?);
    let n_cols = columns.len();
    // Cells covered by a span: (row, col) → (from the left, from above).
    let mut covered = vec![vec![(false, false); n_cols]; n_rows];
    let mut rows = Vec::with_capacity(n_rows);
    for (ri, row) in rows_v.iter().enumerate() {
        let Some(cells_v) = row.as_array() else {
            bail!("table row {} must be an array of cells", ri + 1);
        };
        if cells_v.len() != n_cols {
            bail!(
                "table row {} has {} cells; the table has {n_cols} columns (use null for cells covered by a span)",
                ri + 1,
                cells_v.len()
            );
        }
        let mut cells = Vec::with_capacity(n_cols);
        for (ci, cell_v) in cells_v.iter().enumerate() {
            let is_header = header && ri == 0;
            let band = banded && !is_header && (ri - usize::from(header)) % 2 == 1;
            let mut style = table_style.clone();
            if is_header {
                style = style.over(&RunStyle {
                    bold: Some(true),
                    color: Some(header_color.clone()),
                    ..RunStyle::default()
                });
            }
            let fill_default = if is_header {
                Some(header_fill.clone())
            } else if band {
                Some(band_fill.clone())
            } else {
                body_fill.clone()
            };
            let mut cell = CellSpec {
                text: TextSpec {
                    paras: vec![base_defaults.para(Vec::new(), RunStyle::default())],
                    anchor: table_anchor,
                    wrap: true,
                    ..TextSpec::default()
                },
                style: style.clone(),
                fill: fill_default,
                borders: [
                    Some(border.clone()),
                    Some(border.clone()),
                    Some(border.clone()),
                    Some(border.clone()),
                ],
                margins,
                col_span: 1,
                row_span: 1,
                h_merge: covered[ri][ci].0,
                v_merge: covered[ri][ci].1,
            };
            match cell_v {
                Value::Null => {}
                Value::String(_) | Value::Number(_) | Value::Array(_) => {
                    cell.text.paras = paragraphs(cell_v, &base_defaults, cx)?;
                }
                Value::Object(_) => {
                    let co = object(cell_v, "a table cell", CELL_KEYS)?;
                    let sub = Ctx {
                        style: style.clone(),
                        ..cx.clone()
                    };
                    let (t, st) = text_body(co, &sub, base_defaults.clone(), table_anchor)?;
                    if let Some(t) = t {
                        cell.text.paras = t.paras;
                        cell.text.anchor = t.anchor;
                    }
                    cell.style = st;
                    if let Some(f) = get(co, "fill") {
                        cell.fill = Some(fill(f, "fill")?);
                    }
                    if let Some(b) = get(co, "border") {
                        let b = line(b, cx, "border")?;
                        cell.borders = [Some(b.clone()), Some(b.clone()), Some(b.clone()), Some(b)];
                    }
                    if let Some(m) = get(co, "margins") {
                        cell.margins = Some(four(m, cx, "margins")?);
                    }
                    let span = |k: &str| -> Result<u32> {
                        match get(co, k) {
                            None => Ok(1),
                            Some(v) => v
                                .as_u64()
                                .filter(|&n| n >= 1)
                                .map(|n| n as u32)
                                .with_context(|| format!("`{k}` must be a whole number from 1")),
                        }
                    };
                    cell.col_span = span("col_span")?;
                    cell.row_span = span("row_span")?;
                    if ci + cell.col_span as usize > n_cols || ri + cell.row_span as usize > n_rows
                    {
                        bail!(
                            "the cell in row {}, column {} spans past the table",
                            ri + 1,
                            ci + 1
                        );
                    }
                    let (rows_n, cols_n) = (cell.row_span as usize, cell.col_span as usize);
                    for (r, row) in covered.iter_mut().enumerate().skip(ri).take(rows_n) {
                        for (c, at) in row.iter_mut().enumerate().skip(ci).take(cols_n) {
                            if (r, c) != (ri, ci) {
                                *at = (c > ci, r > ri);
                            }
                        }
                    }
                }
                _ => bail!("a table cell must be text, a number, null or an object"),
            }
            if (cell.h_merge || cell.v_merge) && !cell_v.is_null() {
                bail!(
                    "row {}, column {} is covered by a span: give null there",
                    ri + 1,
                    ci + 1
                );
            }
            caps_text(&mut cell.text.paras, &cell.style);
            // Margins as [left, top, right, bottom] → the writer's [l, r, t, b].
            cell.margins = cell.margins.map(|[l, t, r, b]| [l, r, t, b]);
            cells.push(cell);
        }
        rows.push(RowSpec {
            height: heights[ri],
            cells,
        });
    }
    let style_id = opt_str(o, "style_id")?.map(str::to_owned);
    if let Some(id) = &style_id
        && !(id.starts_with('{') && id.ends_with('}') && id.len() == 38)
    {
        bail!(
            "`style_id` must be a table style GUID such as {{5C22544A-7EE6-4342-B048-85BDC9FD1C3A}}"
        );
    }
    Ok(Element {
        kind: ElementKind::Table(TableSpec {
            columns,
            rows,
            first_row: header,
            band_rows: banded,
            style_id,
        }),
        name: opt_str(o, "name")?.map(str::to_owned),
        xfrm: Xfrm::new(
            opt_len(o, "x", cx)?.unwrap_or(0),
            opt_len(o, "y", cx)?.unwrap_or(0),
            0,
            0,
        ),
    })
}

/// A slide background: a colour, `{"gradient": ...}`, `{"picture":
/// path}`, or `"inherit"` (the layout's and master's).
pub fn background(v: &Value, cx: &Ctx) -> Result<BackgroundSpec> {
    if v.is_null() || v.as_str() == Some("inherit") {
        return Ok(BackgroundSpec::Inherit);
    }
    if let Some(p) = v.get("picture") {
        object(v, "`background`", &["picture"])?;
        let path = p.as_str().context("`picture` must be a file path")?;
        let (path, bytes) = read_file(path, cx)?;
        if slides_engine::image_ext(&bytes).is_none() {
            bail!("{} is not a PNG or JPEG picture", path.display());
        }
        return Ok(BackgroundSpec::Picture(Arc::new(bytes)));
    }
    Ok(BackgroundSpec::Fill(fill(v, "background")?))
}

// ── slides ─────────────────────────────────────────────────────────────────

/// The keys of a slide in a deck (and of `slides_add_slide`).
const SLIDE_KEYS: &[&str] = &[
    "layout",
    "kind",
    "eyebrow",
    "title",
    "subtitle",
    "bullets",
    "background",
    "elements",
    "notes",
    "footer",
    "footer_note",
    "transition",
];

/// Add `elements` (an array) to slide `slide`; returns their ids.
pub fn add_elements(
    p: &mut Presentation,
    slide: usize,
    elements: &Value,
    cx: &Ctx,
) -> Result<Vec<Value>> {
    let Some(list) = elements.as_array() else {
        bail!("`elements` must be an array");
    };
    if list.len() > MAX_ELEMENTS {
        bail!("more than {MAX_ELEMENTS} elements at once");
    }
    let cx = &Ctx {
        slide_no: Some(slide + 1),
        ..cx.clone()
    };
    // Components become several elements each.
    let mut flat = Vec::new();
    for (n, v) in list.iter().enumerate() {
        match super::slides_components::expand(v, cx)
            .with_context(|| format!("element {}", n + 1))?
        {
            Some(els) => flat.extend(els.into_iter().map(|e| (n, e))),
            None => flat.push((n, v.clone())),
        }
    }
    if flat.len() > MAX_ELEMENTS * 4 {
        bail!("the elements make more than {} shapes", MAX_ELEMENTS * 4);
    }
    let mut out = Vec::new();
    for (n, v) in &flat {
        let el = element(v, cx).with_context(|| format!("element {}", n + 1))?;
        let i = p
            .add_element(slide, &el)
            .with_context(|| format!("element {}", n + 1))?;
        let s = &p.slide(slide).shapes[i];
        out.push(json!({"id": s.id, "name": s.name}));
    }
    Ok(out)
}

/// What a slide spec adds beyond its title and bullets: background,
/// elements, notes.
fn apply_slide(
    p: &mut Presentation,
    i: usize,
    o: &Map<String, Value>,
    cx: &Ctx,
) -> Result<Vec<Value>> {
    if let Some(bg) = o.get("background") {
        let bg = super::slides_style::resolve_background(bg, cx);
        p.set_background(i, &background(&bg, cx)?)?;
    }
    let shapes = match get(o, "elements") {
        Some(els) => add_elements(p, i, els, cx)?,
        None => Vec::new(),
    };
    if let Some(n) = get(o, "notes") {
        let n = n.as_str().context("`notes` must be a string")?;
        check_len("the notes", n)?;
        p.set_notes_text(i, n)?;
    }
    Ok(shapes)
}

/// The layout a deck slide asks for, by name or number, or the one its
/// content suits (Blank, Title Only, Title and Content).
fn deck_layout(p: &Presentation, o: &Map<String, Value>) -> Result<Option<usize>> {
    if get(o, "layout").is_some() {
        return super::slides::layout_arg(p, &Value::Object(o.clone()));
    }
    let name = match (get(o, "title").is_some(), get(o, "bullets").is_some()) {
        (_, true) => "Title and Content",
        (true, false) => "Title Only",
        (false, false) => "Blank",
    };
    Ok(p.layouts.iter().position(|l| l.name == name))
}

const DECK_KEYS: &[&str] = &[
    "size",
    "units",
    "preset",
    "theme",
    "palette",
    "styles",
    "text",
    "background",
    "header",
    "footer",
    "cover",
    "content",
    "transition",
    "slides",
];

/// Slide sizes by name.
fn slide_size(v: &Value, cx: &Ctx) -> Result<SlideSize> {
    let size = match v {
        Value::String(s) => match s.as_str() {
            "16:9" | "widescreen" => SlideSize::WIDE,
            "4:3" => SlideSize {
                cx: 9_144_000,
                cy: 6_858_000,
            },
            "16:10" => SlideSize {
                cx: 9_144_000,
                cy: 5_715_000,
            },
            "a4" | "A4" => SlideSize {
                cx: 9_906_000,
                cy: 6_858_000,
            },
            "letter" => SlideSize {
                cx: 9_144_000,
                cy: 6_858_000,
            },
            other => bail!(
                "`size` \"{other}\" is not one of 16:9, 4:3, 16:10, a4, letter; or give {{\"w\", \"h\"}}"
            ),
        },
        Value::Object(_) => {
            let o = object(v, "`size`", &["w", "h"])?;
            SlideSize {
                cx: need_len(o, "w", cx, "`size`")?,
                cy: need_len(o, "h", cx, "`size`")?,
            }
        }
        _ => bail!("`size` must be a name such as \"16:9\" or {{\"w\", \"h\"}}"),
    };
    // PowerPoint's limits: 1 to 56 inches.
    let ok = |v: Emu| (914_400..=51_206_400).contains(&v);
    if !ok(size.cx) || !ok(size.cy) {
        bail!("the slide size must be between 1 and 56 inches each way");
    }
    Ok(size)
}

fn deck_theme(v: &Value) -> Result<DeckTheme> {
    let o = object(v, "`theme`", &["name", "fonts", "colors"])?;
    let mut t = DeckTheme::default();
    if let Some(n) = opt_str(o, "name")? {
        t.name = n.to_owned();
    }
    if let Some(f) = get(o, "fonts") {
        let f = object(f, "`theme.fonts`", &["heading", "body"])?;
        if let Some(h) = opt_str(f, "heading")? {
            t.major_font = h.to_owned();
        }
        if let Some(b) = opt_str(f, "body")? {
            t.minor_font = b.to_owned();
        }
    }
    if let Some(c) = get(o, "colors") {
        let c = object(c, "`theme.colors`", &THEME_COLOR_NAMES)?;
        for (i, name) in THEME_COLOR_NAMES.iter().enumerate() {
            if let Some(v) = get(c, name) {
                match color(v, name)?.base {
                    slides_engine::author::ColorBase::Rgb(rgb) => t.colors[i] = Some(rgb.hex()),
                    _ => bail!("theme colour `{name}` must be #RRGGBB"),
                }
            }
        }
    }
    Ok(t)
}

/// A presentation built from a deck description. Relative picture paths
/// start from `base`.
pub fn build(deck: &Value, base: Option<&Path>) -> Result<Presentation> {
    let o = object(deck, "the deck", DECK_KEYS)?;
    let mut cx = Ctx {
        units: units_arg(deck)?,
        base: base.map(Path::to_path_buf),
        ..Ctx::default()
    };
    let size = get(o, "size")
        .map(|s| slide_size(s, &cx))
        .transpose()?
        .unwrap_or(SlideSize::WIDE);
    cx.size = Some(size);
    let theme = super::slides_style::deck_theme(o)?
        .map(|t| deck_theme(&t))
        .transpose()?
        .unwrap_or_default();
    let mut p = Presentation::new_with(&PackageOptions {
        size,
        theme,
        more_layouts: true,
        title_slide: false,
    })?;
    cx.look = Some(Arc::new(super::slides_style::Look::build(&p, o, &cx)?));
    if let Some(t) = get(o, "text") {
        object(t, "`text`", STYLE_KEYS)?;
        // Palette colours and theme fonts allowed.
        let mut tv = t.clone();
        tv["type"] = json!("text");
        let mut to = super::slides_style::resolve(&tv, &cx)?
            .as_object()
            .cloned()
            .unwrap_or_default();
        to.remove("type");
        cx.style = run_style(&to)?;
    }
    let slides = get(o, "slides")
        .and_then(Value::as_array)
        .context("the deck needs `slides`: an array of slides")?;
    if slides.is_empty() || slides.len() > MAX_SLIDES {
        bail!("a deck has 1 to {MAX_SLIDES} slides");
    }
    let deck_bg = get(o, "background");
    for (n, s) in slides.iter().enumerate() {
        add_deck_slide(&mut p, s, deck_bg, &cx).with_context(|| format!("slide {}", n + 1))?;
    }
    Ok(p)
}

/// Add one deck slide at the end: header (eyebrow, title, subtitle) or a
/// cover, bullets, elements, footer, notes and transition.
fn add_deck_slide(
    p: &mut Presentation,
    s: &Value,
    deck_bg: Option<&Value>,
    cx: &Ctx,
) -> Result<usize> {
    use super::slides_style as style;
    let so = object(s, "a slide", SLIDE_KEYS)?;
    let look = style::look_of(cx);
    let kind = opt_str(so, "kind")?.unwrap_or("content");
    let cover = match kind {
        "content" => false,
        "cover" | "section" => true,
        other => bail!("slide `kind` must be content, cover or section, not \"{other}\""),
    };
    let header_mode = cover || (look.header.is_some() && get(so, "layout").is_none());
    if !header_mode {
        for k in ["eyebrow", "subtitle"] {
            if get(so, k).is_some() {
                bail!("`{k}` needs a deck `header` (or a `preset`), and no `layout` on the slide");
            }
        }
    }
    let layout = if header_mode {
        p.layouts.iter().position(|l| l.name == "Blank")
    } else {
        deck_layout(p, so)?
    };
    let plan = if header_mode {
        super::slides::plan_add(p, &json!({}))?
    } else {
        super::slides::plan_add(
            p,
            &json!({
                "title": so.get("title").cloned().unwrap_or(Value::Null),
                "bullets": so.get("bullets").cloned().unwrap_or(Value::Null),
            }),
        )?
    };
    let i = p.add_slide_with_text(
        p.len().checked_sub(1),
        layout,
        plan.title.as_deref(),
        &plan.body,
    )?;
    let mut cx = Ctx {
        slide_no: Some(i + 1),
        ..cx.clone()
    };
    let mut so = so.clone();
    if !so.contains_key("background") {
        if cover {
            so.insert(
                "background".into(),
                style::cover_background(&look, &cx, kind == "section"),
            );
        } else if let Some(bg) = deck_bg {
            so.insert("background".into(), bg.clone());
        }
    }
    let mut area = look.content;
    if header_mode {
        let f = if cover {
            style::cover(&so, &cx, &look, kind == "section")?
        } else {
            style::header(&so, &cx, &look)?
        };
        for el in &f.elements {
            p.add_element(i, el)?;
        }
        area = f.content;
        if let Some(b) = get(&so, "bullets") {
            let list = b.as_array().context("`bullets` must be an array")?;
            let paras = list
                .iter()
                .map(|x| match x {
                    Value::String(t) => Ok(json!({"text": t})),
                    Value::Object(_) => {
                        let bo = object(x, "a bullet", &["text", "level"])?;
                        let mut q =
                            json!({"text": opt_str(bo, "text")?.context("a bullet needs `text`")?});
                        if let Some(l) = get(bo, "level") {
                            q["level"] = l.clone();
                        }
                        Ok(q)
                    }
                    _ => bail!("a bullet must be a string or {{text, level}}"),
                })
                .collect::<Result<Vec<Value>>>()?;
            let v = json!({"type": "text", "name": "Content", "style": "body", "bullet": true, "space_before": 8,
                "x": format!("{}emu", area[0]), "y": format!("{}emu", area[1]),
                "w": format!("{}emu", area[2]), "h": format!("{}emu", area[3]), "text": paras});
            p.add_element(i, &element(&v, &cx)?)?;
        }
    }
    cx.area = Some(area);
    apply_slide(p, i, &so, &cx)?;
    let footer = match get(&so, "footer") {
        None => !cover,
        Some(Value::Bool(b)) => *b,
        Some(_) => bail!("a slide's `footer` is true or false"),
    };
    if footer {
        for el in style::footer(&so, &cx, &look)? {
            p.add_element(i, &el)?;
        }
    }
    let transition = match get(&so, "transition") {
        Some(t) => style::transition(t)?,
        None => look.transition.clone(),
    };
    if let Some(t) = transition {
        p.set_transition(i, Some(&t))?;
    }
    Ok(i)
}

// ── tools ──────────────────────────────────────────────────────────────────

/// The slides tools that change a presentation ([`run_tool`]).
pub const EDIT_TOOLS: &[&str] = &[
    "slides_add_slide",
    "slides_set_shape_text",
    "slides_delete_slide",
    "slides_set_notes",
    "slides_add_shape",
    "slides_add_picture",
    "slides_add_table",
    "slides_add_elements",
    "slides_set_background",
    "slides_update_shape",
    "slides_delete_shape",
    "slides_order_shape",
    "slides_group_shapes",
];

fn slide_of(p: &Presentation, args: &Value) -> Result<usize> {
    super::slides::slide_arg(p, args, "slide")
}

fn shape_of(p: &Presentation, slide: usize, args: &Value, key: &str) -> Result<usize> {
    let id = super::req_int(args, key)?;
    let id = u32::try_from(id).with_context(|| format!("bad `{key}`"))?;
    p.shape_index(slide, id).with_context(|| {
        format!(
            "slide {} has no shape with id {id}; see slides_list",
            slide + 1
        )
    })
}

/// The element fields of a single-element tool's arguments.
fn element_args(args: &Value, ty: Option<&str>) -> Value {
    let mut el = args.as_object().cloned().unwrap_or_default();
    for k in ["document", "slide", "units"] {
        el.remove(k);
    }
    if let Some(ty) = ty {
        el.insert("type".into(), json!(ty));
    } else if !el.contains_key("type") {
        el.insert("type".into(), json!("shape"));
    }
    Value::Object(el)
}

/// Run an editing slides tool on `p` (`current`: the slide on screen).
/// Returns what was done, the answer, and where the change is.
pub fn run_tool(
    p: &mut Presentation,
    tool: &str,
    args: &Value,
    base: Option<&Path>,
) -> Result<(String, Value, Focus)> {
    let cx = Ctx::for_tool(p, args, base)?;
    Ok(match tool {
        "slides_add_slide" => {
            let mut a = args.as_object().cloned().unwrap_or_default();
            let extra: Map<String, Value> = ["background", "elements", "notes"]
                .iter()
                .filter_map(|k| a.remove(*k).map(|v| ((*k).to_owned(), v)))
                .collect();
            let plan = super::slides::plan_add(p, &Value::Object(a))?;
            let i =
                p.add_slide_with_text(plan.after, plan.layout, plan.title.as_deref(), &plan.body)?;
            let shapes = apply_slide(p, i, &extra, &cx)?;
            let mut out = json!({"slide": i + 1});
            if !shapes.is_empty() {
                out["shapes"] = json!(shapes);
            }
            (format!("added slide {}", i + 1), out, Focus::slide(i))
        }
        "slides_set_shape_text" => {
            let i = slide_of(p, args)?;
            let ix = shape_of(p, i, args, "shape_id")?;
            let text = super::req_str(args, "text")?;
            check_len("`text`", text)?;
            p.set_shape_text((i, ix), text)?;
            let id = p.slide(i).shapes[ix].id;
            (
                format!("changed shape {id} on slide {}", i + 1),
                json!({}),
                Focus::shape(i, ix),
            )
        }
        "slides_delete_slide" => {
            let i = slide_of(p, args)?;
            if p.len() == 1 {
                bail!("that is the only slide; add another before deleting it");
            }
            p.delete_slide(i)?;
            let next = i.min(p.len() - 1);
            (
                format!("deleted slide {}", i + 1),
                json!({"slides_now": p.len()}),
                Focus::slide(next),
            )
        }
        "slides_set_notes" => {
            let i = slide_of(p, args)?;
            // `notes`, as in a deck's slides, or `text`.
            let text = match (
                super::opt_str(args, "text")?,
                super::opt_str(args, "notes")?,
            ) {
                (Some(_), Some(_)) => bail!("give `text` or `notes`, not both"),
                (Some(t), None) | (None, Some(t)) => t,
                (None, None) => bail!("`text` (or `notes`) is required"),
            };
            check_len("`text`", text)?;
            p.set_notes_text(i, text)?;
            (
                format!("changed the notes of slide {}", i + 1),
                json!({}),
                Focus::slide(i),
            )
        }
        "slides_add_shape" | "slides_add_picture" | "slides_add_table" | "slides_add_elements" => {
            let i = slide_of(p, args)?;
            let shapes = if tool == "slides_add_elements" {
                let els = args.get("elements").context("`elements` is required")?;
                add_elements(p, i, els, &cx)?
            } else {
                let ty = match tool {
                    "slides_add_picture" => Some("picture"),
                    "slides_add_table" => Some("table"),
                    _ => None,
                };
                // One element (a clean table: its header bar too).
                let one = json!([element_args(args, ty)]);
                add_elements(p, i, &one, &cx)?
            };
            let last = p.slide(i).shapes.len().saturating_sub(1);
            let focus = if shapes.len() == 1 {
                Focus::shape(i, last)
            } else {
                Focus::slide(i)
            };
            (
                format!("added {} shape(s) to slide {}", shapes.len(), i + 1),
                json!({"slide": i + 1, "shapes": shapes}),
                focus,
            )
        }
        "slides_set_background" => {
            let i = slide_of(p, args)?;
            let bg = args
                .get("background")
                .context("`background` is required (a colour, a gradient, {\"picture\": path} or \"inherit\")")?;
            p.set_background(i, &background(bg, &cx)?)?;
            (
                format!("changed the background of slide {}", i + 1),
                json!({}),
                Focus::slide(i),
            )
        }
        "slides_update_shape" => {
            let i = slide_of(p, args)?;
            let ix = shape_of(p, i, args, "shape_id")?;
            let shape = &p.slide(i).shapes[ix];
            let mut x = shape
                .xfrm
                .or_else(|| p.placed((i, ix)).map(|pl| pl.xfrm))
                .context("this shape has no position of its own")?;
            let o = args.as_object().context("arguments")?;
            for (k, field) in [("x", 0), ("y", 1), ("w", 2), ("h", 3)] {
                if let Some(v) = opt_len(o, k, &cx)? {
                    match field {
                        0 => x.x = v,
                        1 => x.y = v,
                        2 => x.cx = v.max(1),
                        _ => x.cy = v.max(1),
                    }
                }
            }
            if let Some(deg) = opt_num(o, "rotation")? {
                x = x.with_degrees(deg);
            }
            if let Some(f) = opt_bool(o, "flip_h")? {
                x.flip_h = f;
            }
            if let Some(f) = opt_bool(o, "flip_v")? {
                x.flip_v = f;
            }
            p.set_xfrm((i, ix), x, false)?;
            let id = p.slide(i).shapes[ix].id;
            (
                format!("moved shape {id} on slide {}", i + 1),
                json!({"x": x.x, "y": x.y, "w": x.cx, "h": x.cy}),
                Focus::shape(i, ix),
            )
        }
        "slides_delete_shape" => {
            let i = slide_of(p, args)?;
            let ix = shape_of(p, i, args, "shape_id")?;
            let id = p.slide(i).shapes[ix].id;
            p.delete_shape((i, ix))?;
            (
                format!("deleted shape {id} on slide {}", i + 1),
                json!({}),
                Focus::slide(i),
            )
        }
        "slides_order_shape" => {
            let i = slide_of(p, args)?;
            let ix = shape_of(p, i, args, "shape_id")?;
            let n = p.slide(i).shapes.len();
            let to = match args.get("to") {
                Some(Value::String(s)) => match s.as_str() {
                    "front" => n - 1,
                    "back" => 0,
                    "forward" => (ix + 1).min(n - 1),
                    "backward" => ix.saturating_sub(1),
                    other => bail!(
                        "`to` must be front, back, forward, backward or a position, not \"{other}\""
                    ),
                },
                Some(Value::Number(v)) => match v.as_u64() {
                    Some(k) if k >= 1 => (k as usize - 1).min(n - 1),
                    _ => bail!("`to` as a position counts from 1 (the back)"),
                },
                _ => bail!(
                    "`to` is required: front, back, forward, backward or a position from 1 (the back)"
                ),
            };
            let at = p.reorder_shape(i, ix, to)?;
            let id = p.slide(i).shapes[at].id;
            (
                format!("moved shape {id} on slide {} to position {}", i + 1, at + 1),
                json!({"position": at + 1, "of": n}),
                Focus::shape(i, at),
            )
        }
        "slides_group_shapes" => {
            let i = slide_of(p, args)?;
            let ids = args
                .get("shape_ids")
                .and_then(Value::as_array)
                .context("`shape_ids` is required: two or more shape ids")?;
            if ids.len() < 2 {
                bail!("group two or more shapes");
            }
            let idx = ids
                .iter()
                .map(|v| shape_of(p, i, &json!({"id": v}), "id"))
                .collect::<Result<Vec<_>>>()?;
            let g = p.group_shapes(i, &idx)?;
            let s = &p.slide(i).shapes[g];
            (
                format!("grouped {} shapes on slide {}", idx.len(), i + 1),
                json!({"id": s.id, "name": s.name}),
                Focus::shape(i, g),
            )
        }
        other => bail!("unknown tool {other}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cx() -> Ctx {
        Ctx::default()
    }

    #[std::prelude::v1::test]
    fn reads_lengths_colours_and_fills() {
        let c = Ctx {
            units: Unit::In,
            ..Ctx::default()
        };
        assert_eq!(length(&json!(1), &c, "x").unwrap(), 914_400);
        assert_eq!(length(&json!("2.54cm"), &c, "x").unwrap(), 914_400);
        assert_eq!(length(&json!("72pt"), &cx(), "x").unwrap(), 914_400);
        assert_eq!(length(&json!(5), &cx(), "x").unwrap(), 5);
        assert!(length(&json!("3 parsecs"), &cx(), "x").is_err());
        assert_eq!(
            color(&json!("accent1"), "c").unwrap(),
            Color::scheme("accent1")
        );
        let c8 = color(&json!("#FF000080"), "c").unwrap();
        assert_eq!(c8.alpha, Some(50_196));
        assert_eq!(
            color(&json!("#abc"), "c").unwrap(),
            Color::rgb(Rgb::parse("AABBCC").unwrap())
        );
        assert!(color(&json!("blurple"), "c").is_err());
        assert_eq!(fill(&json!("none"), "f").unwrap(), FillSpec::None);
        let g = fill(&json!({"gradient": {"angle": 45, "stops": ["#000000", {"color": "#FFFFFF", "position": "80%"}]}}), "f").unwrap();
        let FillSpec::Gradient { stops, kind } = g else {
            panic!()
        };
        assert_eq!(stops[1].0, 80_000);
        assert_eq!(kind, GradientKind::Linear(45.0));
        let l = line(
            &json!({"color": "#123456", "width": "2pt", "dash": "dash", "tail": "stealth"}),
            &cx(),
            "line",
        )
        .unwrap();
        assert_eq!(l.width, Some(25_400));
        assert_eq!(l.tail.unwrap().kind, "stealth");
        assert!(line(&json!({"colour": "#123456"}), &cx(), "line").is_err());
    }

    #[std::prelude::v1::test]
    fn reads_shapes_with_runs_and_rounded_corners() {
        let el = element(
            &json!({
                "type": "shape", "geometry": "roundRect", "radius": "0.25in",
                "x": "1in", "y": "1in", "w": "3in", "h": "1in",
                "fill": "#1F3864", "line": "none",
                "font": "Inter", "size": 14, "color": "#FFFFFF",
                "text": [
                    {"runs": [{"text": "Bold", "bold": true}, " plain"], "align": "left"},
                    {"text": "Second", "size": 10, "bullet": true}
                ],
                "anchor": "top", "insets": "0.1in", "autofit": "shrink", "line_spacing": 1.1
            }),
            &cx(),
        )
        .unwrap();
        let ElementKind::Shape(s) = &el.kind else {
            panic!()
        };
        assert_eq!(s.geom.adjust, vec![("adj".to_owned(), 25_000)]);
        let t = s.text.as_ref().unwrap();
        assert_eq!(t.paras.len(), 2);
        assert_eq!(t.paras[0].align, Some(Align::Left));
        assert_eq!(t.paras[1].align, Some(Align::Center));
        assert_eq!(t.paras[0].runs[0].style.bold, Some(true));
        assert_eq!(t.paras[1].runs[0].style.size, Some(1000));
        assert_eq!(t.paras[1].line_spacing, Some(Spacing::Pct(110_000)));
        assert_eq!(t.anchor, Some(Anchor::Top));
        assert_eq!(t.autofit, Autofit::Shrink);
        assert_eq!(s.style.font.as_deref(), Some("Inter"));
        assert!(
            element(
                &json!({"type": "shape", "w": 1, "h": 1, "geometry": "nope"}),
                &cx()
            )
            .is_err()
        );
        assert!(element(&json!({"type": "shape", "w": 1}), &cx()).is_err());
        assert!(element(&json!({"type": "blob"}), &cx()).is_err());
    }

    #[std::prelude::v1::test]
    fn reads_lines_and_tables() {
        let el = element(
            &json!({"type": "line", "x1": 100, "y1": 500, "x2": 300, "y2": 100, "color": "#FF0000", "width": "1.5pt", "tail": {"type": "triangle", "width": "large"}}),
            &cx(),
        )
        .unwrap();
        assert_eq!(el.xfrm.x, 100);
        assert_eq!(el.xfrm.cy, 400);
        assert!(el.xfrm.flip_v && !el.xfrm.flip_h);
        let el = element(
            &json!({
                "type": "table", "x": 0, "y": 0, "w": 3000, "columns": [1, 2],
                "rows": [["Name", "Score"], ["Ann", 9], [{"text": "Total", "col_span": 2}, null]],
                "banded": true
            }),
            &cx(),
        )
        .unwrap();
        let ElementKind::Table(t) = &el.kind else {
            panic!()
        };
        assert_eq!(t.columns, vec![1000, 2000]);
        assert_eq!(t.rows[0].cells[0].style.bold, Some(true));
        assert!(t.rows[2].cells[1].h_merge);
        assert_eq!(t.rows[2].cells[0].col_span, 2);
        assert!(
            element(
                &json!({"type": "table", "w": 10, "rows": [["a", "b"], ["c"]]}),
                &cx()
            )
            .is_err()
        );
    }

    #[std::prelude::v1::test]
    fn tools_add_move_order_and_delete_shapes() {
        let mut p = Presentation::new();
        let (_, out, _) = run_tool(
            &mut p,
            "slides_add_shape",
            &json!({"slide": 1, "units": "in", "geometry": "chevron", "x": 1, "y": 1, "w": 2, "h": 1, "text": "Step"}),
            None,
        )
        .unwrap();
        let id = out["shapes"][0]["id"].as_i64().unwrap();
        run_tool(
            &mut p,
            "slides_update_shape",
            &json!({"slide": 1, "shape_id": id, "x": "2in", "rotation": 90}),
            None,
        )
        .unwrap();
        let ix = p.shape_index(0, id as u32).unwrap();
        let x = p.slide(0).shapes[ix].xfrm.unwrap();
        assert_eq!((x.x, x.y, x.rot), (1_828_800, 914_400, 5_400_000));
        let (_, out, _) = run_tool(
            &mut p,
            "slides_order_shape",
            &json!({"slide": 1, "shape_id": id, "to": "back"}),
            None,
        )
        .unwrap();
        assert_eq!(out["position"], 1);
        assert_eq!(p.slide(0).shapes[0].id, id as u32);
        run_tool(
            &mut p,
            "slides_set_background",
            &json!({"slide": 1, "background": {"gradient": {"stops": ["#000000", "#333333"]}}}),
            None,
        )
        .unwrap();
        run_tool(
            &mut p,
            "slides_delete_shape",
            &json!({"slide": 1, "shape_id": id}),
            None,
        )
        .unwrap();
        assert!(p.shape_index(0, id as u32).is_none());
        assert!(
            run_tool(
                &mut p,
                "slides_add_picture",
                &json!({"slide": 1, "path": "rel.png"}),
                None
            )
            .is_err()
        );
    }

    fn example(name: &str) -> (Value, PathBuf) {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/examples/slides");
        let deck: Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join(name)).unwrap()).unwrap();
        (deck, dir)
    }

    fn shapes(listed: &Value, slide: usize) -> Vec<Value> {
        listed["slides"][slide]["shapes"]
            .as_array()
            .unwrap()
            .clone()
    }

    #[std::prelude::v1::test]
    fn builds_the_example_deck_the_same_every_time() {
        let (deck, dir) = example("deck.json");
        let p = build(&deck, Some(&dir)).unwrap();
        let bytes = p.to_bytes().unwrap();
        assert_eq!(build(&deck, Some(&dir)).unwrap().to_bytes().unwrap(), bytes);
        assert!(slides_engine::package_problems(&bytes).is_empty());

        // Read back: the same slides, shapes and geometry as the JSON says.
        let q = Presentation::from_bytes(bytes).unwrap();
        assert_eq!(q.len(), deck["slides"].as_array().unwrap().len());
        let listed = super::super::slides::list(&q, 0);
        // The cover: a centred title placeholder over a picture.
        assert_eq!(
            listed["slides"][0]["title"],
            "On time, for the third quarter running"
        );
        assert!(q.slide(0).background.is_some());
        assert!(
            !shapes(&listed, 0)
                .iter()
                .any(|s| s["name"] == "Slide number")
        );
        // Header: eyebrow in capitals, the title as the title placeholder.
        let s2 = &listed["slides"][1];
        assert_eq!(s2["title"], "How a shipment moves");
        let s2s = shapes(&listed, 1);
        assert!(s2s.iter().any(|s| s["text"] == "HOW IT WORKS"));
        assert!(
            s2s.iter()
                .any(|s| s["placeholder"] == "title" && s["text"] == "How a shipment moves")
        );
        assert_eq!(s2s.iter().filter(|s| s["geometry"] == "chevron").count(), 3);
        assert!(s2s.iter().any(|s| s["geometry"] == "homePlate"));
        // Footer: bar, logo, text and a slide number field.
        let number = s2s.iter().find(|s| s["name"] == "Slide number").unwrap();
        assert_eq!(number["text"], "2");
        assert!(
            s2s.iter()
                .any(|s| s["name"] == "Logo" && s["kind"] == "picture")
        );
        let s3 = shapes(&listed, 2);
        assert_eq!(
            s3.iter()
                .filter(|s| s["name"].as_str().is_some_and(|n| n.ends_with(" value")))
                .count(),
            3
        );
        assert!(
            s3.iter()
                .any(|s| s["text"] == "Figures are invented for this example")
        );
        assert!(shapes(&listed, 3).iter().any(|s| s["kind"] == "table"));
        let pics: Vec<Value> = shapes(&listed, 4)
            .into_iter()
            .filter(|s| s["kind"] == "picture" && s["name"] != "Logo")
            .collect();
        assert_eq!(pics.len(), 3);
        assert_eq!(pics[0]["geometry"], "roundRect");
        assert_eq!(pics[1]["geometry"], "ellipse");
        assert!(
            shapes(&listed, 5)
                .iter()
                .any(|s| s["name"] == "Card 3 badge icon")
        );
        assert_eq!(q.notes_text(5), "Ask for approval of the six-week pilot.");
        // Transitions: the deck's fade, the last slide's push.
        let fx = q.slide(1).show_fx().transition.unwrap();
        assert_eq!(fx.duration_ms, 600);
        assert!(matches!(
            q.slide(5).show_fx().transition.unwrap().kind,
            slides_engine::anim::TransitionKind::Push(_)
        ));
        assert_eq!(q.size, SlideSize::WIDE);
    }

    #[std::prelude::v1::test]
    fn builds_the_proposal_example() {
        let (deck, dir) = example("proposal.json");
        let p = build(&deck, Some(&dir)).unwrap();
        let bytes = p.to_bytes().unwrap();
        assert!(slides_engine::package_problems(&bytes).is_empty());
        let q = Presentation::from_bytes(bytes).unwrap();
        let listed = super::super::slides::list(&q, 0);
        for (i, s) in deck["slides"].as_array().unwrap().iter().enumerate() {
            assert_eq!(listed["slides"][i]["title"], s["title"], "slide {}", i + 1);
        }
        // Every element is ordinary, editable shapes.
        let all: Vec<Value> = (0..q.len()).flat_map(|i| shapes(&listed, i)).collect();
        assert!(
            all.iter()
                .all(|s| s.get("text_locked").is_none() || s["name"] == "Slide number")
        );
    }

    #[std::prelude::v1::test]
    fn styles_caps_fields_and_notes_alias() {
        let deck = json!({
            "units": "in",
            "styles": {"kicker": {"size": 9, "caps": true, "letter_spacing": 2, "color": "highlight"}},
            "slides": [{"elements": [
                {"type": "text", "x": 1, "y": 1, "w": 4, "h": 1, "style": "kicker", "text": "Hello"},
                {"type": "text", "x": 1, "y": 2, "w": 4, "h": 1, "text": [{"runs": ["Page ", {"field": "slide_number", "bold": true}]}]},
                {"type": "icon", "icon": "rocket", "x": 1, "y": 3, "size": 0.5}
            ]}]
        });
        let mut p = build(&deck, None).unwrap();
        let listed = super::super::slides::list(&p, 0);
        let sh = shapes(&listed, 0);
        assert_eq!(sh[0]["text"], "HELLO");
        assert_eq!(sh[1]["text"], "Page 1");
        assert_eq!(sh[2]["name"], "Icon: rocket");
        let ElementKind::Shape(s) = element(
            &json!({"type": "text", "w": 1, "h": 1, "letter_spacing": 2, "text": "x"}),
            &Ctx::default(),
        )
        .unwrap()
        .kind
        else {
            panic!()
        };
        assert_eq!(s.style.spacing, Some(200));
        assert!(build(&json!({"slides": [{"eyebrow": "x"}]}), None).is_err());
        assert!(build(&json!({"slides": [{"footer_note": "x"}]}), None).is_err());
        run_tool(
            &mut p,
            "slides_set_notes",
            &json!({"slide": 1, "notes": "Hi"}),
            None,
        )
        .unwrap();
        assert_eq!(p.notes_text(0), "Hi");
        assert!(run_tool(&mut p, "slides_set_notes", &json!({"slide": 1}), None).is_err());
        let (_, out, _) = run_tool(
            &mut p,
            "slides_add_elements",
            &json!({"slide": 1, "units": "in", "elements": [{"type": "callout", "title": "Note:", "text": "components work in tools"}]}),
            None,
        )
        .unwrap();
        assert_eq!(out["shapes"].as_array().unwrap().len(), 1);
    }

    #[std::prelude::v1::test]
    fn deck_errors_name_the_slide_and_element() {
        let e = build(
            &json!({"slides": [{}, {"elements": [{"type": "shape", "w": 1, "h": 1}, {"type": "shape", "w": 1}]}]}),
            None,
        )
        .err()
        .unwrap();
        let msg = format!("{e:#}");
        assert!(
            msg.contains("slide 2") && msg.contains("element 2"),
            "{msg}"
        );
        assert!(build(&json!({"slides": [], "size": "4:3"}), None).is_err());
        assert!(build(&json!({"slides": [{}], "sise": "4:3"}), None).is_err());
        let p = build(&json!({"size": "4:3", "slides": [{"title": "Hi"}]}), None).unwrap();
        assert_eq!(p.size.cx, 9_144_000);
    }
}
