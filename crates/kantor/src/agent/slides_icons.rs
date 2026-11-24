//! Icons in decks (`{"type": "icon", "icon": "shield-check"}`): a Lucide
//! icon (the catalog GPUI Kit ships, MIT/ISC licensed) turned into an
//! editable DrawingML shape. Lucide icons are strokes on a 24 × 24 grid, so
//! the shape is a custom geometry with no fill whose outline is the icon:
//! recolour it like any line in PowerPoint.

use std::fmt::Write as _;

use anyhow::{Context as _, Result, bail};
use gpui_kit::AssetSource as _;
use serde_json::Value;
use slides_engine::Xfrm;
use slides_engine::author::*;

use super::slides_author::{Ctx, color, keys_with, object, opt_len, opt_num, opt_str};

/// Path units per icon unit (Lucide's grid is 24 units).
const SCALE: f64 = 1000.0;

/// The SVG of Lucide icon `name` (`shield-check`, `ShieldCheck` and
/// `shield_check` all work).
pub fn svg(name: &str) -> Result<String> {
    let file = kebab(name);
    if file.is_empty() || !file.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        bail!("`icon` \"{name}\" is not an icon name such as \"shield-check\"");
    }
    let path = format!("icons/{file}.svg");
    match gpui_kit::assets::AllAssets.load(&path) {
        Ok(Some(bytes)) => Ok(String::from_utf8_lossy(&bytes).into_owned()),
        _ => {
            let close = similar(&file);
            if close.is_empty() {
                bail!("there is no Lucide icon \"{file}\" (see https://lucide.dev/icons)");
            }
            bail!(
                "there is no Lucide icon \"{file}\"; similar: {} (see https://lucide.dev/icons)",
                close.join(", ")
            )
        }
    }
}

/// `ShieldCheck` / `shield_check` → `shield-check`.
fn kebab(name: &str) -> String {
    let mut out = String::new();
    let mut prev_lower = false;
    for c in name.trim().chars() {
        if c == '_' || c == ' ' {
            out.push('-');
            prev_lower = false;
        } else if c.is_ascii_uppercase() {
            if prev_lower {
                out.push('-');
            }
            out.push(c.to_ascii_lowercase());
            prev_lower = false;
        } else {
            out.push(c);
            prev_lower = c.is_ascii_lowercase() || c.is_ascii_digit();
        }
    }
    out
}

/// Up to six catalog names sharing a word with `name`.
fn similar(name: &str) -> Vec<String> {
    let words: Vec<&str> = name.split('-').filter(|w| w.len() > 2).collect();
    let Ok(all) = gpui_kit::assets::AllAssets.list("icons/") else {
        return Vec::new();
    };
    let mut out: Vec<String> = all
        .iter()
        .filter_map(|p| {
            let p: &str = p.as_ref();
            let n = p.strip_prefix("icons/")?.strip_suffix(".svg")?;
            n.split('-')
                .any(|w| words.contains(&w))
                .then(|| n.to_owned())
        })
        .collect();
    out.sort();
    out.truncate(6);
    out
}

// ── SVG → DrawingML path ───────────────────────────────────────────────────

/// The attributes of the SVG elements in `svg`: (tag, [(name, value)]).
fn svg_elements(svg: &str) -> Vec<(String, Vec<(String, String)>)> {
    let mut out = Vec::new();
    let mut rest = svg;
    while let Some(start) = rest.find('<') {
        rest = &rest[start + 1..];
        let Some(end) = rest.find('>') else {
            break;
        };
        let tag = &rest[..end];
        rest = &rest[end + 1..];
        if tag.starts_with('/') || tag.starts_with('!') || tag.starts_with('?') {
            continue;
        }
        let name: String = tag
            .chars()
            .take_while(|c| !c.is_whitespace() && *c != '/')
            .collect();
        let mut attrs = Vec::new();
        let mut a = &tag[name.len()..];
        while let Some(eq) = a.find('=') {
            let key = a[..eq].trim().trim_start_matches('/').trim().to_owned();
            let after = a[eq + 1..].trim_start();
            let Some(q) = after.chars().next().filter(|c| *c == '"' || *c == '\'') else {
                break;
            };
            let Some(close) = after[1..].find(q) else {
                break;
            };
            attrs.push((key, after[1..1 + close].to_owned()));
            a = &after[close + 2..];
        }
        out.push((name, attrs));
    }
    out
}

/// A path being built in icon units.
#[derive(Default)]
struct Path {
    out: String,
    /// Current point and the start of the subpath.
    at: (f64, f64),
    start: (f64, f64),
    /// The last control point (for `S` / `T`).
    last_ctrl: Option<(f64, f64)>,
    last_quad: Option<(f64, f64)>,
}

fn pt(out: &mut String, (x, y): (f64, f64)) {
    let _ = write!(
        out,
        "<a:pt x=\"{}\" y=\"{}\"/>",
        (x * SCALE).round() as i64,
        (y * SCALE).round() as i64
    );
}

impl Path {
    fn move_to(&mut self, p: (f64, f64)) {
        self.out.push_str("<a:moveTo>");
        pt(&mut self.out, p);
        self.out.push_str("</a:moveTo>");
        self.at = p;
        self.start = p;
        self.last_ctrl = None;
        self.last_quad = None;
    }

    fn line_to(&mut self, p: (f64, f64)) {
        self.out.push_str("<a:lnTo>");
        pt(&mut self.out, p);
        self.out.push_str("</a:lnTo>");
        self.at = p;
        self.last_ctrl = None;
        self.last_quad = None;
    }

    fn cubic(&mut self, c1: (f64, f64), c2: (f64, f64), p: (f64, f64)) {
        self.out.push_str("<a:cubicBezTo>");
        pt(&mut self.out, c1);
        pt(&mut self.out, c2);
        pt(&mut self.out, p);
        self.out.push_str("</a:cubicBezTo>");
        self.at = p;
        self.last_ctrl = Some(c2);
        self.last_quad = None;
    }

    fn quad(&mut self, c: (f64, f64), p: (f64, f64)) {
        self.out.push_str("<a:quadBezTo>");
        pt(&mut self.out, c);
        pt(&mut self.out, p);
        self.out.push_str("</a:quadBezTo>");
        self.at = p;
        self.last_ctrl = None;
        self.last_quad = Some(c);
    }

    fn close(&mut self) {
        self.out.push_str("<a:close/>");
        self.at = self.start;
        self.last_ctrl = None;
        self.last_quad = None;
    }

    /// An SVG elliptical arc to `p`, as cubic Béziers.
    fn arc(&mut self, rx: f64, ry: f64, rot_deg: f64, large: bool, sweep: bool, p: (f64, f64)) {
        let (x1, y1) = self.at;
        let (x2, y2) = p;
        if (x1 - x2).abs() < 1e-9 && (y1 - y2).abs() < 1e-9 {
            return;
        }
        let (mut rx, mut ry) = (rx.abs(), ry.abs());
        if rx < 1e-9 || ry < 1e-9 {
            self.line_to(p);
            return;
        }
        let phi = rot_deg.to_radians();
        let (sin, cos) = phi.sin_cos();
        let dx = (x1 - x2) / 2.0;
        let dy = (y1 - y2) / 2.0;
        let x1p = cos * dx + sin * dy;
        let y1p = -sin * dx + cos * dy;
        let lambda = (x1p * x1p) / (rx * rx) + (y1p * y1p) / (ry * ry);
        if lambda > 1.0 {
            let s = lambda.sqrt();
            rx *= s;
            ry *= s;
        }
        let num = rx * rx * ry * ry - rx * rx * y1p * y1p - ry * ry * x1p * x1p;
        let den = rx * rx * y1p * y1p + ry * ry * x1p * x1p;
        let mut coef = if den.abs() < 1e-12 {
            0.0
        } else {
            (num / den).max(0.0).sqrt()
        };
        if large == sweep {
            coef = -coef;
        }
        let cxp = coef * rx * y1p / ry;
        let cyp = -coef * ry * x1p / rx;
        let cx = cos * cxp - sin * cyp + (x1 + x2) / 2.0;
        let cy = sin * cxp + cos * cyp + (y1 + y2) / 2.0;
        let angle = |ux: f64, uy: f64, vx: f64, vy: f64| {
            let a = (ux * vy - uy * vx).atan2(ux * vx + uy * vy);
            if a.is_finite() { a } else { 0.0 }
        };
        let theta1 = angle(1.0, 0.0, (x1p - cxp) / rx, (y1p - cyp) / ry);
        let mut delta = angle(
            (x1p - cxp) / rx,
            (y1p - cyp) / ry,
            (-x1p - cxp) / rx,
            (-y1p - cyp) / ry,
        );
        let tau = std::f64::consts::TAU;
        if !sweep && delta > 0.0 {
            delta -= tau;
        } else if sweep && delta < 0.0 {
            delta += tau;
        }
        let segs = (delta.abs() / (std::f64::consts::FRAC_PI_2))
            .ceil()
            .max(1.0) as usize;
        let step = delta / segs as f64;
        let k = 4.0 / 3.0 * (step / 4.0).tan();
        let point = |t: f64| {
            let (st, ct) = t.sin_cos();
            (
                cx + rx * ct * cos - ry * st * sin,
                cy + rx * ct * sin + ry * st * cos,
            )
        };
        let deriv = |t: f64| {
            let (st, ct) = t.sin_cos();
            (
                -rx * st * cos - ry * ct * sin,
                -rx * st * sin + ry * ct * cos,
            )
        };
        let mut t = theta1;
        for i in 0..segs {
            let t2 = t + step;
            let (p1, d1) = (point(t), deriv(t));
            let (p2, d2) = (point(t2), deriv(t2));
            let end = if i + 1 == segs { p } else { p2 };
            self.cubic(
                (p1.0 + k * d1.0, p1.1 + k * d1.1),
                (p2.0 - k * d2.0, p2.1 - k * d2.1),
                end,
            );
            t = t2;
        }
    }

    fn ellipse(&mut self, cx: f64, cy: f64, rx: f64, ry: f64) {
        self.move_to((cx + rx, cy));
        self.arc(rx, ry, 0.0, false, true, (cx, cy + ry));
        self.arc(rx, ry, 0.0, false, true, (cx - rx, cy));
        self.arc(rx, ry, 0.0, false, true, (cx, cy - ry));
        self.arc(rx, ry, 0.0, false, true, (cx + rx, cy));
        self.close();
    }

    fn rect(&mut self, x: f64, y: f64, w: f64, h: f64, rx: f64, ry: f64) {
        let rx = rx.clamp(0.0, w / 2.0);
        let ry = ry.clamp(0.0, h / 2.0);
        if rx <= 0.0 || ry <= 0.0 {
            self.move_to((x, y));
            self.line_to((x + w, y));
            self.line_to((x + w, y + h));
            self.line_to((x, y + h));
            self.close();
            return;
        }
        self.move_to((x + rx, y));
        self.line_to((x + w - rx, y));
        self.arc(rx, ry, 0.0, false, true, (x + w, y + ry));
        self.line_to((x + w, y + h - ry));
        self.arc(rx, ry, 0.0, false, true, (x + w - rx, y + h));
        self.line_to((x + rx, y + h));
        self.arc(rx, ry, 0.0, false, true, (x, y + h - ry));
        self.line_to((x, y + ry));
        self.arc(rx, ry, 0.0, false, true, (x + rx, y));
        self.close();
    }

    fn points(&mut self, list: &str, close: bool) {
        let nums = numbers(list);
        let mut it = nums.chunks_exact(2);
        if let Some(first) = it.next() {
            self.move_to((first[0], first[1]));
            for p in it {
                self.line_to((p[0], p[1]));
            }
            if close {
                self.close();
            }
        }
    }

    /// SVG path data (`d`).
    fn data(&mut self, d: &str) -> Result<()> {
        let mut t = Tokens::new(d);
        let mut cmd = ' ';
        while !t.done() {
            match t.command() {
                Some(c) => cmd = c,
                // Repeated arguments; after a moveto they are linetos.
                None => match cmd {
                    'M' => cmd = 'L',
                    'm' => cmd = 'l',
                    ' ' | 'Z' | 'z' => bail!("bad path data"),
                    _ => {}
                },
            }
            let rel = cmd.is_ascii_lowercase();
            let (ox, oy) = if rel { self.at } else { (0.0, 0.0) };
            match cmd.to_ascii_uppercase() {
                'M' => {
                    let p = (ox + t.num()?, oy + t.num()?);
                    self.move_to(p);
                }
                'L' => {
                    let p = (ox + t.num()?, oy + t.num()?);
                    self.line_to(p);
                }
                'H' => {
                    let x = t.num()? + if rel { self.at.0 } else { 0.0 };
                    self.line_to((x, self.at.1));
                }
                'V' => {
                    let y = t.num()? + if rel { self.at.1 } else { 0.0 };
                    self.line_to((self.at.0, y));
                }
                'C' => {
                    let c1 = (ox + t.num()?, oy + t.num()?);
                    let c2 = (ox + t.num()?, oy + t.num()?);
                    let p = (ox + t.num()?, oy + t.num()?);
                    self.cubic(c1, c2, p);
                }
                'S' => {
                    let c1 = match self.last_ctrl {
                        Some((cx, cy)) => (2.0 * self.at.0 - cx, 2.0 * self.at.1 - cy),
                        None => self.at,
                    };
                    let c2 = (ox + t.num()?, oy + t.num()?);
                    let p = (ox + t.num()?, oy + t.num()?);
                    self.cubic(c1, c2, p);
                }
                'Q' => {
                    let c = (ox + t.num()?, oy + t.num()?);
                    let p = (ox + t.num()?, oy + t.num()?);
                    self.quad(c, p);
                }
                'T' => {
                    let c = match self.last_quad {
                        Some((cx, cy)) => (2.0 * self.at.0 - cx, 2.0 * self.at.1 - cy),
                        None => self.at,
                    };
                    let p = (ox + t.num()?, oy + t.num()?);
                    self.quad(c, p);
                }
                'A' => {
                    let (rx, ry, rot) = (t.num()?, t.num()?, t.num()?);
                    let large = t.flag().context("bad arc flag")?;
                    let sweep = t.flag().context("bad arc flag")?;
                    let p = (ox + t.num()?, oy + t.num()?);
                    self.arc(rx, ry, rot, large, sweep, p);
                }
                'Z' => self.close(),
                other => bail!("unknown path command {other}"),
            }
        }
        Ok(())
    }
}

/// Numbers in a list such as `"1,2 3.5-4"`.
fn numbers(s: &str) -> Vec<f64> {
    let mut t = Tokens::new(s);
    let mut out = Vec::new();
    while let Some(n) = t.number() {
        out.push(n);
    }
    out
}

struct Tokens<'a> {
    s: &'a [u8],
    i: usize,
}

impl<'a> Tokens<'a> {
    fn new(s: &'a str) -> Self {
        Self {
            s: s.as_bytes(),
            i: 0,
        }
    }

    fn skip(&mut self) {
        while self.i < self.s.len()
            && (self.s[self.i].is_ascii_whitespace() || self.s[self.i] == b',')
        {
            self.i += 1;
        }
    }

    fn done(&mut self) -> bool {
        self.skip();
        self.i >= self.s.len()
    }

    fn command_next(&mut self) -> bool {
        self.skip();
        self.s
            .get(self.i)
            .is_some_and(|c| c.is_ascii_alphabetic() && *c != b'e' && *c != b'E')
    }

    fn command(&mut self) -> Option<char> {
        if self.command_next() {
            self.i += 1;
            Some(self.s[self.i - 1] as char)
        } else {
            None
        }
    }

    fn num(&mut self) -> Result<f64> {
        self.number().context("bad path data")
    }

    fn flag(&mut self) -> Option<bool> {
        self.skip();
        let c = *self.s.get(self.i)?;
        self.i += 1;
        match c {
            b'0' => Some(false),
            b'1' => Some(true),
            _ => None,
        }
    }

    fn number(&mut self) -> Option<f64> {
        self.skip();
        let start = self.i;
        let s = self.s;
        let mut i = self.i;
        if i < s.len() && (s[i] == b'-' || s[i] == b'+') {
            i += 1;
        }
        let mut dot = false;
        let mut digits = false;
        while i < s.len() {
            match s[i] {
                b'0'..=b'9' => {
                    digits = true;
                    i += 1;
                }
                b'.' if !dot => {
                    dot = true;
                    i += 1;
                }
                _ => break,
            }
        }
        if digits && i < s.len() && (s[i] == b'e' || s[i] == b'E') {
            let mut j = i + 1;
            if j < s.len() && (s[j] == b'-' || s[j] == b'+') {
                j += 1;
            }
            if j < s.len() && s[j].is_ascii_digit() {
                i = j;
                while i < s.len() && s[i].is_ascii_digit() {
                    i += 1;
                }
            }
        }
        if !digits {
            return None;
        }
        self.i = i;
        std::str::from_utf8(&s[start..i]).ok()?.parse().ok()
    }
}

/// The `a:custGeom` of an icon's SVG, and its grid size.
pub fn cust_geom(svg: &str) -> Result<String> {
    let els = svg_elements(svg);
    let attr = |attrs: &[(String, String)], k: &str| -> Option<f64> {
        attrs.iter().find(|(n, _)| n == k)?.1.trim().parse().ok()
    };
    let (mut vw, mut vh) = (24.0, 24.0);
    let mut path = Path::default();
    for (tag, attrs) in &els {
        let a = |k: &str| attr(attrs, k).unwrap_or(0.0);
        let s = |k: &str| {
            attrs
                .iter()
                .find(|(n, _)| n == k)
                .map(|(_, v)| v.as_str())
                .unwrap_or("")
        };
        match tag.as_str() {
            "svg" => {
                let vb = numbers(s("viewBox"));
                if vb.len() == 4 && vb[2] > 0.0 && vb[3] > 0.0 {
                    (vw, vh) = (vb[2], vb[3]);
                }
            }
            "path" => path.data(s("d"))?,
            "circle" => path.ellipse(a("cx"), a("cy"), a("r"), a("r")),
            "ellipse" => path.ellipse(a("cx"), a("cy"), a("rx"), a("ry")),
            "line" => {
                path.move_to((a("x1"), a("y1")));
                path.line_to((a("x2"), a("y2")));
            }
            "rect" => {
                let rx = attr(attrs, "rx").or(attr(attrs, "ry")).unwrap_or(0.0);
                let ry = attr(attrs, "ry").unwrap_or(rx);
                path.rect(a("x"), a("y"), a("width"), a("height"), rx, ry);
            }
            "polyline" => path.points(s("points"), false),
            "polygon" => path.points(s("points"), true),
            _ => {}
        }
    }
    if path.out.is_empty() {
        bail!("the icon has no shapes");
    }
    Ok(format!(
        "<a:custGeom><a:avLst/><a:gdLst/><a:ahLst/><a:cxnLst/><a:rect l=\"l\" t=\"t\" r=\"r\" b=\"b\"/><a:pathLst><a:path w=\"{}\" h=\"{}\" fill=\"none\">{}</a:path></a:pathLst></a:custGeom>",
        (vw * SCALE).round() as i64,
        (vh * SCALE).round() as i64,
        path.out
    ))
}

// ── the element ────────────────────────────────────────────────────────────

pub const ICON_KEYS: &[&str] = &[
    "type", "name", "icon", "x", "y", "w", "h", "size", "color", "stroke", "rotation",
];

/// `{"type": "icon", "icon": "shield-check", "x", "y", "size" | "w"/"h",
/// "color", "stroke"}`.
pub fn icon_element(v: &Value, cx: &Ctx) -> Result<Element> {
    let o = object(v, "an icon", &keys_with(&[ICON_KEYS]))?;
    let name = opt_str(o, "icon")?
        .context("an icon needs `icon`: a Lucide name such as \"shield-check\"")?;
    let geom = cust_geom(&svg(name)?)?;
    let size = opt_len(o, "size", cx)?;
    let w = opt_len(o, "w", cx)?.or(size).or(opt_len(o, "h", cx)?);
    let h = opt_len(o, "h", cx)?.or(size).or(w);
    let (Some(w), Some(h)) = (w, h) else {
        bail!("an icon needs `size` (or `w` / `h`)");
    };
    let mut xfrm = Xfrm::new(
        opt_len(o, "x", cx)?.unwrap_or(0),
        opt_len(o, "y", cx)?.unwrap_or(0),
        w.max(1),
        h.max(1),
    );
    if let Some(deg) = opt_num(o, "rotation")? {
        xfrm = xfrm.with_degrees(deg);
    }
    // Lucide's strokes are 2 units of 24.
    let stroke = match opt_len(o, "stroke", cx)? {
        Some(s) => s.max(1),
        None => ((w.min(h) as f64) / 12.0).round().max(3_175.0) as i64,
    };
    let color = match o.get("color").filter(|c| !c.is_null()) {
        Some(c) => color(c, "color")?,
        None => Color::scheme("accent1"),
    };
    Ok(Element {
        kind: ElementKind::Shape(ShapeSpec {
            geom: GeomSpec::custom(geom),
            text_box: false,
            fill: Some(FillSpec::None),
            line: Some(LineSpec {
                fill: Some(FillSpec::Solid(color)),
                width: Some(stroke),
                round_cap: true,
                ..LineSpec::default()
            }),
            text: None,
            style: RunStyle::default(),
            placeholder: None,
        }),
        name: Some(
            opt_str(o, "name")?
                .map(str::to_owned)
                .unwrap_or_else(|| format!("Icon: {}", kebab(name))),
        ),
        xfrm,
    })
}

/// The JSON Schema of an icon element.
pub fn schema() -> Value {
    let r = |n: &str| serde_json::json!({"$ref": format!("#/$defs/{n}")});
    serde_json::json!({
        "type": "object", "additionalProperties": false, "required": ["type", "icon"],
        "description": "A Lucide icon (https://lucide.dev/icons) as an editable outline shape: recolour it like a line.",
        "properties": {
            "type": {"const": "icon"},
            "name": {"type": "string"},
            "icon": {"type": "string", "description": "The icon's name, e.g. shield-check, rocket, chart-line, users."},
            "x": r("length"), "y": r("length"),
            "size": {"description": "Width and height (or give w / h).", "allOf": [r("length")]},
            "w": r("length"), "h": r("length"),
            "color": {"description": "Default accent1.", "allOf": [r("color")]},
            "stroke": {"description": "Line width (default a twelfth of the size, as Lucide's).", "allOf": [r("length")]},
            "rotation": {"type": "number"},
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[std::prelude::v1::test]
    fn icons_become_stroked_custom_shapes() {
        let svg = svg("ShieldCheck").unwrap();
        let g = cust_geom(&svg).unwrap();
        assert!(g.starts_with("<a:custGeom>") && g.ends_with("</a:custGeom>"));
        assert!(g.contains("<a:path w=\"24000\" h=\"24000\" fill=\"none\">"));
        assert!(g.contains("<a:cubicBezTo>"));
        assert!(svg_elements("<svg><circle cx=\"12\" cy=\"12\" r=\"10\" /></svg>").len() == 2);
        let c = cust_geom(r#"<svg viewBox="0 0 24 24"><circle cx="12" cy="12" r="10"/><rect x="3" y="3" width="18" height="18" rx="2"/><path d="m9 12 2 2 4-4M3 3h2v2zA1 1 0 0 1 4 4"/><polyline points="1 2 3 4"/></svg>"#).unwrap();
        assert!(c.contains("<a:pt x=\"22000\" y=\"12000\"/>"), "{c}");
        assert!(c.contains("<a:close/>"));
        assert!(svg("no-such-icon-anywhere").is_err());
        assert!(svg("../secrets").is_err());
        let el = icon_element(
            &serde_json::json!({"type": "icon", "icon": "rocket", "x": 0, "y": 0, "size": 914400, "color": "#FF0000"}),
            &Ctx::default(),
        )
        .unwrap();
        el.check().unwrap();
        let ElementKind::Shape(s) = &el.kind else {
            panic!()
        };
        assert_eq!(s.line.as_ref().unwrap().width, Some(76_200));
        assert_eq!(el.xfrm.cx, 914_400);
    }

    #[std::prelude::v1::test]
    fn every_catalog_icon_converts() {
        let all = gpui_kit::assets::AllAssets.list("icons/").unwrap();
        assert!(all.len() > 1000);
        for p in all {
            let p: String = AsRef::<str>::as_ref(&p).to_owned();
            let bytes = gpui_kit::assets::AllAssets.load(&p).unwrap().unwrap();
            let svg = String::from_utf8_lossy(&bytes);
            cust_geom(&svg).unwrap_or_else(|e| panic!("{p}: {e}"));
        }
    }
}
