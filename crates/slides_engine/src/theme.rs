//! Read-only display inheritance: theme colours and fonts, the master's
//! colour map, and list styles (text defaults per outline level).

use std::collections::HashMap;

use docs_engine::Rgb;
use roxmltree::Node;

use crate::model::Align;
use crate::xml::*;

/// Theme colours (`a:clrScheme`) and fonts (`a:fontScheme`).
#[derive(Clone, Debug, Default)]
pub struct Theme {
    colors: HashMap<String, Rgb>,
    pub major_font: Option<String>,
    pub minor_font: Option<String>,
}

impl Theme {
    pub fn parse(xml: &str) -> Self {
        let mut t = Self::default();
        let Ok(doc) = roxmltree::Document::parse(xml) else {
            return t;
        };
        for n in doc.descendants() {
            if is(n, NS_A, "clrScheme") {
                for slot in n.children().filter(|c| c.is_element()) {
                    let name = slot.tag_name().name().to_owned();
                    let c = slot.children().find(|c| c.is_element()).and_then(|c| {
                        match c.tag_name().name() {
                            "srgbClr" => c.attribute("val").and_then(Rgb::parse),
                            "sysClr" => c.attribute("lastClr").and_then(Rgb::parse),
                            _ => None,
                        }
                    });
                    if let Some(c) = c {
                        t.colors.insert(name, c);
                    }
                }
            } else if is(n, NS_A, "majorFont") {
                t.major_font = path(n, &[(NS_A, "latin")])
                    .and_then(|l| l.attribute("typeface"))
                    .map(str::to_owned);
            } else if is(n, NS_A, "minorFont") {
                t.minor_font = path(n, &[(NS_A, "latin")])
                    .and_then(|l| l.attribute("typeface"))
                    .map(str::to_owned);
            }
        }
        t
    }

    /// A typeface name with theme references (`+mj-lt`, `+mn-lt`) resolved.
    pub fn font(&self, face: &str) -> Option<String> {
        match face {
            "+mj-lt" | "+mj-ea" | "+mj-cs" => self.major_font.clone(),
            "+mn-lt" | "+mn-ea" | "+mn-cs" => self.minor_font.clone(),
            "" => None,
            f => Some(f.to_owned()),
        }
    }
}

/// The master's `p:clrMap` (`tx1` → `dk1`, ...).
#[derive(Clone, Debug)]
pub struct ClrMap(HashMap<String, String>);

impl Default for ClrMap {
    fn default() -> Self {
        let pairs = [
            ("bg1", "lt1"),
            ("tx1", "dk1"),
            ("bg2", "lt2"),
            ("tx2", "dk2"),
        ];
        Self(
            pairs
                .iter()
                .map(|(a, b)| (a.to_string(), b.to_string()))
                .collect(),
        )
    }
}

impl ClrMap {
    pub fn parse(n: Node) -> Self {
        let mut m = Self::default();
        for a in n.attributes() {
            m.0.insert(a.name().to_owned(), a.value().to_owned());
        }
        m
    }
}

/// What colours resolve against.
#[derive(Clone, Copy)]
pub struct ColorCtx<'a> {
    pub theme: &'a Theme,
    pub map: &'a ClrMap,
    /// `phClr` inside style matrix references.
    pub ph: Option<Rgb>,
}

impl ColorCtx<'_> {
    fn scheme(&self, name: &str) -> Option<Rgb> {
        if name == "phClr" {
            return self.ph;
        }
        let slot = self.map.0.get(name).map(String::as_str).unwrap_or(name);
        self.theme.colors.get(slot).copied()
    }

    /// A colour element (`a:srgbClr`, `a:schemeClr`, ...) with `lumMod` /
    /// `lumOff` / `tint` / `shade` applied (approximately).
    pub fn color(&self, n: Node) -> Option<Rgb> {
        let base = match n.tag_name().name() {
            "srgbClr" => n.attribute("val").and_then(Rgb::parse),
            "sysClr" => n.attribute("lastClr").and_then(Rgb::parse),
            "schemeClr" => self.scheme(n.attribute("val")?),
            "prstClr" => match n.attribute("val")? {
                "black" => Some(Rgb([0, 0, 0])),
                "white" => Some(Rgb([255, 255, 255])),
                "red" => Some(Rgb([255, 0, 0])),
                "blue" => Some(Rgb([0, 0, 255])),
                "green" => Some(Rgb([0, 128, 0])),
                "yellow" => Some(Rgb([255, 255, 0])),
                "gray" | "grey" => Some(Rgb([128, 128, 128])),
                _ => None,
            },
            "scrgbClr" => {
                let c = |k| (attr_i64(n, k).unwrap_or(0) as f32 / 100_000.0 * 255.0) as u8;
                Some(Rgb([c("r"), c("g"), c("b")]))
            }
            _ => None,
        }?;
        let mut hsl = to_hsl(base);
        let mut rgb = base;
        let mut touched_hsl = false;
        for m in n.children().filter(|c| c.is_element()) {
            let v = attr_i64(m, "val").unwrap_or(100_000) as f32 / 100_000.0;
            match m.tag_name().name() {
                "lumMod" => {
                    hsl.2 *= v;
                    touched_hsl = true;
                }
                "lumOff" => {
                    hsl.2 += v;
                    touched_hsl = true;
                }
                "tint" => {
                    let c = if touched_hsl { from_hsl(hsl) } else { rgb };
                    rgb = Rgb(c
                        .0
                        .map(|x| (x as f32 + (255.0 - x as f32) * (1.0 - v)) as u8));
                    hsl = to_hsl(rgb);
                    touched_hsl = false;
                }
                "shade" => {
                    let c = if touched_hsl { from_hsl(hsl) } else { rgb };
                    rgb = Rgb(c.0.map(|x| (x as f32 * v) as u8));
                    hsl = to_hsl(rgb);
                    touched_hsl = false;
                }
                _ => {}
            }
        }
        Some(if touched_hsl { from_hsl(hsl) } else { rgb })
    }

    /// The colour of a fill-like parent (`a:solidFill`, a `fillRef`, ...):
    /// its first colour child.
    pub fn first_color(&self, n: Node) -> Option<Rgb> {
        n.children()
            .filter(|c| c.is_element())
            .find_map(|c| self.color(c))
    }

    /// Display colour of a fill element (`a:solidFill`, `a:gradFill` → its
    /// first stop). `None` for no fill / pictures / patterns.
    pub fn fill(&self, n: Node) -> Option<Rgb> {
        match n.tag_name().name() {
            "solidFill" => self.first_color(n),
            "gradFill" => n
                .descendants()
                .find(|d| is(*d, NS_A, "gs"))
                .and_then(|gs| self.first_color(gs)),
            "pattFill" => path(n, &[(NS_A, "fgClr")]).and_then(|c| self.first_color(c)),
            _ => None,
        }
    }
}

fn to_hsl(c: Rgb) -> (f32, f32, f32) {
    let [r, g, b] = c.0.map(|x| x as f32 / 255.0);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) / 2.0;
    if max == min {
        return (0.0, 0.0, l);
    }
    let d = max - min;
    let s = if l > 0.5 {
        d / (2.0 - max - min)
    } else {
        d / (max + min)
    };
    let h = if max == r {
        (g - b) / d + if g < b { 6.0 } else { 0.0 }
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };
    (h / 6.0, s, l)
}

fn from_hsl((h, s, l): (f32, f32, f32)) -> Rgb {
    let l = l.clamp(0.0, 1.0);
    let s = s.clamp(0.0, 1.0);
    if s == 0.0 {
        let v = (l * 255.0).round() as u8;
        return Rgb([v, v, v]);
    }
    let q = if l < 0.5 {
        l * (1.0 + s)
    } else {
        l + s - l * s
    };
    let p = 2.0 * l - q;
    let hue = |mut t: f32| {
        if t < 0.0 {
            t += 1.0;
        }
        if t > 1.0 {
            t -= 1.0;
        }
        if t < 1.0 / 6.0 {
            p + (q - p) * 6.0 * t
        } else if t < 0.5 {
            q
        } else if t < 2.0 / 3.0 {
            p + (q - p) * (2.0 / 3.0 - t) * 6.0
        } else {
            p
        }
    };
    let c = |t: f32| (hue(t) * 255.0).round() as u8;
    Rgb([c(h + 1.0 / 3.0), c(h), c(h - 1.0 / 3.0)])
}

/// Text defaults of one outline level (`a:lvlNpPr` + its `a:defRPr`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LevelStyle {
    pub align: Option<Align>,
    /// Hundredths of a point.
    pub size: Option<u32>,
    pub bold: Option<bool>,
    pub italic: Option<bool>,
    pub underline: Option<bool>,
    pub color: Option<Rgb>,
    /// Typeface, theme references resolved.
    pub font: Option<String>,
}

impl LevelStyle {
    pub fn overlay(&mut self, o: &LevelStyle) {
        macro_rules! take {
            ($($f:ident),*) => {$( if o.$f.is_some() { self.$f = o.$f.clone(); } )*};
        }
        take!(align, size, bold, italic, underline, color, font);
    }
}

/// Nine levels of a list style (`a:lstStyle`, `p:titleStyle`, ...).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ListStyle(pub [LevelStyle; 9]);

impl ListStyle {
    pub fn parse(n: Node, cx: &ColorCtx) -> Self {
        let mut ls = Self::default();
        for c in n.children().filter(|c| c.is_element()) {
            let name = c.tag_name().name();
            let lvl = name
                .strip_prefix("lvl")
                .and_then(|s| s.strip_suffix("pPr"))
                .and_then(|d| d.parse::<usize>().ok());
            if let Some(l) = lvl.filter(|l| (1..=9).contains(l)) {
                ls.0[l - 1] = level_style(c, cx);
            }
        }
        ls
    }

    pub fn overlay(&mut self, o: &ListStyle) {
        for (a, b) in self.0.iter_mut().zip(&o.0) {
            a.overlay(b);
        }
    }
}

/// Paragraph-level defaults from a `pPr`-like element and its `defRPr`.
pub fn level_style(ppr: Node, cx: &ColorCtx) -> LevelStyle {
    let mut s = LevelStyle {
        align: ppr.attribute("algn").and_then(Align::from_attr),
        ..LevelStyle::default()
    };
    if let Some(d) = child(ppr, NS_A, "defRPr") {
        run_style(d, cx, &mut s);
    }
    s
}

/// Character defaults from an `rPr`-like element.
pub fn run_style(rpr: Node, cx: &ColorCtx, s: &mut LevelStyle) {
    if let Some(sz) = attr_i64(rpr, "sz") {
        s.size = Some(sz.max(1) as u32);
    }
    if let Some(b) = attr_bool(rpr, "b") {
        s.bold = Some(b);
    }
    if let Some(i) = attr_bool(rpr, "i") {
        s.italic = Some(i);
    }
    if let Some(u) = rpr.attribute("u") {
        s.underline = Some(u != "none");
    }
    if let Some(f) = child(rpr, NS_A, "solidFill") {
        s.color = cx.fill(f);
    }
    if let Some(f) = child(rpr, NS_A, "latin").and_then(|l| l.attribute("typeface")) {
        s.font = cx.theme.font(f);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hsl_round_trip_and_lum_mod() {
        for c in [[68, 114, 196], [0, 0, 0], [255, 255, 255], [237, 125, 49]] {
            let back = from_hsl(to_hsl(Rgb(c)));
            for i in 0..3 {
                assert!(
                    (back.0[i] as i32 - c[i] as i32).abs() <= 1,
                    "{c:?} {back:?}"
                );
            }
        }
        let theme = Theme::parse(
            r#"<a:theme xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><a:themeElements>
            <a:clrScheme name="x"><a:dk1><a:sysClr val="windowText" lastClr="000000"/></a:dk1>
            <a:lt1><a:srgbClr val="FFFFFF"/></a:lt1><a:accent1><a:srgbClr val="4472C4"/></a:accent1></a:clrScheme>
            <a:fontScheme name="x"><a:majorFont><a:latin typeface="Calibri Light"/></a:majorFont>
            <a:minorFont><a:latin typeface="Calibri"/></a:minorFont></a:fontScheme></a:themeElements></a:theme>"#,
        );
        let map = ClrMap::default();
        let cx = ColorCtx {
            theme: &theme,
            map: &map,
            ph: None,
        };
        let doc = roxmltree::Document::parse(
            r#"<a:solidFill xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><a:schemeClr val="tx1"><a:lumMod val="50000"/><a:lumOff val="50000"/></a:schemeClr></a:solidFill>"#,
        )
        .unwrap();
        assert_eq!(cx.fill(doc.root_element()), Some(Rgb([128, 128, 128])));
        assert_eq!(theme.font("+mj-lt").as_deref(), Some("Calibri Light"));
    }
}
