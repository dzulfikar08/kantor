//! Formatting of OpenDocument spreadsheets (`.ods`): calamine brings the
//! values and formulas; this module reads the look from the package and
//! applies it to the IronCalc model, as [`crate::xls`] does for `.xls`:
//!
//! * cell styles (named ones of `styles.xml` and the automatic ones of
//!   `content.xml`, with their parents): font (name, size, bold, italic,
//!   underline, strikethrough, colour), background colour, borders,
//!   horizontal and vertical alignment, wrap, indent, rotation, shrink to
//!   fit, protection, and number formats (`number:*-style`, with the
//!   sections their `style:map`s add) turned into Excel format codes;
//! * a column's default cell style, column widths, row heights, hidden
//!   rows and columns, merged cells;
//! * hidden sheets and tab colours (table styles); frozen panes, gridlines
//!   and the active sheet (`settings.xml`).
//!
//! Not read: conditional formats, validation, notes, charts and pictures,
//! page setup. Runs of cells repeated beyond [`MAX_RUN`] (a style given to
//! the rest of a sheet) are left unformatted.

use std::collections::HashMap;
use std::io::Read as _;
use std::path::Path;

use anyhow::{Result, anyhow};
use ironcalc_base::Model;
use ironcalc_base::types::{
    Alignment, Border, BorderItem, BorderStyle, CellProtection, Col, Color, Fill, Font, FontScheme,
    HorizontalAlignment, MergedCell, Row, SheetState, Style, VerticalAlignment,
};
use roxmltree::{Document, Node};

use crate::{MAX_COLS, MAX_ROWS};

const NS_TABLE: &str = "urn:oasis:names:tc:opendocument:xmlns:table:1.0";
const NS_STYLE: &str = "urn:oasis:names:tc:opendocument:xmlns:style:1.0";
const NS_FO: &str = "urn:oasis:names:tc:opendocument:xmlns:xsl-fo-compatible:1.0";
const NS_NUMBER: &str = "urn:oasis:names:tc:opendocument:xmlns:datastyle:1.0";
const NS_OFFICE: &str = "urn:oasis:names:tc:opendocument:xmlns:office:1.0";
const NS_SVG: &str = "urn:oasis:names:tc:opendocument:xmlns:svg-compatible:1.0";
const NS_CONFIG: &str = "urn:oasis:names:tc:opendocument:xmlns:config:1.0";
const NS_LOEXT: &str = "urn:org:documentfoundation:names:experimental:office:xmlns:loext:1.0";

/// Cells formatted at most per repeated run (rows × columns).
const MAX_RUN: u64 = 20_000;
/// Repeated columns or rows given a width, height or visibility at most.
const MAX_LINES: u32 = 2_000;

/// What a cell style sets; `None` inherits.
#[derive(Debug, Clone, Default, PartialEq)]
struct Look {
    font: Option<String>,
    size: Option<f64>,
    bold: Option<bool>,
    italic: Option<bool>,
    underline: Option<bool>,
    strike: Option<bool>,
    color: Option<Option<String>>,
    fill: Option<Option<String>>,
    /// left, right, top, bottom.
    border: [Option<Option<BorderItem>>; 4],
    halign: Option<HorizontalAlignment>,
    valign: Option<VerticalAlignment>,
    wrap: Option<bool>,
    indent: Option<u32>,
    rotation: Option<u32>,
    shrink: Option<bool>,
    protection: Option<(bool, bool)>,
    data_style: Option<String>,
}

impl Look {
    /// `self` with `child`'s settings on top.
    fn with(&self, child: &Look) -> Look {
        fn pick<T: Clone>(a: &Option<T>, b: &Option<T>) -> Option<T> {
            b.clone().or_else(|| a.clone())
        }
        Look {
            font: pick(&self.font, &child.font),
            size: pick(&self.size, &child.size),
            bold: pick(&self.bold, &child.bold),
            italic: pick(&self.italic, &child.italic),
            underline: pick(&self.underline, &child.underline),
            strike: pick(&self.strike, &child.strike),
            color: pick(&self.color, &child.color),
            fill: pick(&self.fill, &child.fill),
            border: [0, 1, 2, 3].map(|i| pick(&self.border[i], &child.border[i])),
            halign: pick(&self.halign, &child.halign),
            valign: pick(&self.valign, &child.valign),
            wrap: pick(&self.wrap, &child.wrap),
            indent: pick(&self.indent, &child.indent),
            rotation: pick(&self.rotation, &child.rotation),
            shrink: pick(&self.shrink, &child.shrink),
            protection: pick(&self.protection, &child.protection),
            data_style: pick(&self.data_style, &child.data_style),
        }
    }
}

/// A length (`1.85in`, `2.5cm`, `10pt`...) in points.
fn points(s: &str) -> Option<f64> {
    let s = s.trim();
    let split = s.find(|c: char| c.is_ascii_alphabetic())?;
    let (n, unit) = s.split_at(split);
    let n: f64 = n.trim().parse().ok()?;
    let k = match unit {
        "pt" => 1.0,
        "in" => 72.0,
        "cm" => 72.0 / 2.54,
        "mm" => 72.0 / 25.4,
        "pc" => 12.0,
        "px" => 0.75,
        _ => return None,
    };
    let v = n * k;
    v.is_finite().then_some(v)
}

/// `#rrggbb` as IronCalc's `#RRGGBB`.
fn color(s: &str) -> Option<String> {
    let hex = s.trim().strip_prefix('#')?;
    (hex.len() == 6 && hex.chars().all(|c| c.is_ascii_hexdigit()))
        .then(|| format!("#{}", hex.to_ascii_uppercase()))
}

/// A border (`0.74pt solid #000000`, `none`): `None` for no line.
fn border(s: &str) -> Option<BorderItem> {
    let mut width = None;
    let mut kind = None;
    let mut rgb = None;
    for part in s.split_whitespace() {
        if part == "none" || part == "hidden" {
            return None;
        } else if let Some(c) = color(part) {
            rgb = Some(c);
        } else if let Some(w) = points(part) {
            width = Some(w);
        } else {
            kind = Some(part);
        }
    }
    let w = width.unwrap_or(0.75);
    let heavy = w >= 1.5;
    let style = match kind.unwrap_or("solid") {
        "double" | "double-thin" => BorderStyle::Double,
        "dotted" => BorderStyle::Dotted,
        "dashed" | "fine-dashed" | "long-dash" => {
            if heavy {
                BorderStyle::MediumDashed
            } else {
                BorderStyle::Dashed
            }
        }
        "dash-dot" => {
            if heavy {
                BorderStyle::MediumDashDot
            } else {
                BorderStyle::DashDot
            }
        }
        "dash-dot-dot" => {
            if heavy {
                BorderStyle::MediumDashDotDot
            } else {
                BorderStyle::DashDotDot
            }
        }
        _ if w <= 0.1 => BorderStyle::Hair,
        _ if w < 1.5 => BorderStyle::Thin,
        _ if w < 2.3 => BorderStyle::Medium,
        _ => BorderStyle::Thick,
    };
    Some(BorderItem {
        style,
        color: rgb.map_or(Color::None, Color::Rgb),
    })
}

fn attr<'a>(n: Node<'a, '_>, ns: &str, name: &str) -> Option<&'a str> {
    n.attribute((ns, name))
}

fn child<'a, 'i>(n: Node<'a, 'i>, ns: &str, name: &str) -> Option<Node<'a, 'i>> {
    n.children()
        .find(|c| c.tag_name().namespace() == Some(ns) && c.tag_name().name() == name)
}

/// The look a `style:style` sets itself.
fn read_look(s: Node, fonts: &HashMap<String, String>) -> Look {
    let mut l = Look {
        data_style: attr(s, NS_STYLE, "data-style-name").map(str::to_owned),
        ..Look::default()
    };
    if let Some(t) = child(s, NS_STYLE, "text-properties") {
        l.font = attr(t, NS_STYLE, "font-name")
            .map(|n| fonts.get(n).cloned().unwrap_or_else(|| n.to_owned()))
            .or_else(|| attr(t, NS_FO, "font-family").map(|f| f.trim_matches('\'').to_owned()));
        l.size = attr(t, NS_FO, "font-size").and_then(points);
        l.bold = attr(t, NS_FO, "font-weight")
            .map(|w| w == "bold" || w.parse::<u32>().is_ok_and(|n| n >= 600));
        l.italic = attr(t, NS_FO, "font-style").map(|s| s == "italic" || s == "oblique");
        l.underline = attr(t, NS_STYLE, "text-underline-style").map(|s| s != "none");
        l.strike = attr(t, NS_STYLE, "text-line-through-style").map(|s| s != "none");
        if attr(t, NS_STYLE, "use-window-font-color") == Some("true") {
            l.color = Some(None);
        }
        if let Some(c) = attr(t, NS_FO, "color") {
            l.color = Some(color(c));
        }
    }
    if let Some(c) = child(s, NS_STYLE, "table-cell-properties") {
        if let Some(bg) = attr(c, NS_FO, "background-color") {
            l.fill = Some(color(bg));
        }
        if let Some(all) = attr(c, NS_FO, "border") {
            let b = border(all);
            l.border = [0, 1, 2, 3].map(|_| Some(b.clone()));
        }
        for (i, side) in ["border-left", "border-right", "border-top", "border-bottom"]
            .into_iter()
            .enumerate()
        {
            if let Some(v) = attr(c, NS_FO, side) {
                l.border[i] = Some(border(v));
            }
        }
        l.wrap = attr(c, NS_FO, "wrap-option").map(|w| w == "wrap");
        l.valign = attr(c, NS_STYLE, "vertical-align").map(|v| match v {
            "top" => VerticalAlignment::Top,
            "middle" => VerticalAlignment::Center,
            _ => VerticalAlignment::Bottom,
        });
        l.shrink = attr(c, NS_STYLE, "shrink-to-fit").map(|v| v == "true");
        l.rotation = attr(c, NS_STYLE, "rotation-angle")
            .and_then(|a| a.trim_end_matches("deg").parse::<f64>().ok())
            .map(|a| {
                // Excel: 0-90 up, 91-180 down (90 + degrees below).
                let a = a.rem_euclid(360.0).round() as u32;
                match a {
                    0..=90 => a,
                    270..=359 => 90 + (360 - a),
                    _ => 0,
                }
            });
        l.protection = attr(c, NS_STYLE, "cell-protect").map(|p| match p {
            "none" => (false, false),
            "hidden-and-protected" => (true, true),
            "formula-hidden" | "protected formula-hidden" => (true, true),
            _ => (true, false),
        });
    }
    if let Some(p) = child(s, NS_STYLE, "paragraph-properties") {
        l.halign = attr(p, NS_FO, "text-align").map(|a| match a {
            "start" | "left" => HorizontalAlignment::Left,
            "center" => HorizontalAlignment::Center,
            "end" | "right" => HorizontalAlignment::Right,
            "justify" => HorizontalAlignment::Justify,
            _ => HorizontalAlignment::General,
        });
        // An indent level is about 10 points (3 spaces of the default font).
        l.indent = attr(p, NS_FO, "margin-left")
            .and_then(points)
            .map(|m| (m / 7.5).round().clamp(0.0, 250.0) as u32);
    }
    // A cell whose alignment comes from its value type has none of its own.
    if let Some(c) = child(s, NS_STYLE, "table-cell-properties")
        && attr(c, NS_STYLE, "text-align-source") == Some("value-type")
    {
        l.halign = Some(HorizontalAlignment::General);
    }
    l
}

// ------------------------------------------------------------------ number formats

fn quote(text: &str, out: &mut String) {
    // Characters a format code shows as they are.
    const PLAIN: &str = " -/():+$%";
    if text.chars().all(|c| PLAIN.contains(c)) {
        out.push_str(text);
    } else {
        out.push('"');
        out.push_str(&text.replace('"', "\\\""));
        out.push('"');
    }
}

fn integer_part(min: usize, grouping: bool) -> String {
    let mut digits = "0".repeat(min);
    if digits.is_empty() {
        digits.push('#');
    }
    if grouping {
        while digits.len() < 4 {
            digits.insert(0, '#');
        }
        let at = digits.len() - 3;
        digits.insert(at, ',');
    }
    digits
}

fn decimals(n: Node) -> String {
    let places: usize = attr(n, NS_NUMBER, "decimal-places")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let min: usize = attr(n, NS_NUMBER, "min-decimal-places")
        .and_then(|v| v.parse().ok())
        .unwrap_or(places)
        .min(places);
    if places == 0 {
        return String::new();
    }
    format!(".{}{}", "0".repeat(min), "#".repeat(places - min))
}

const COLOR_NAMES: [(&str, &str); 8] = [
    ("#000000", "[BLACK]"),
    ("#0000FF", "[BLUE]"),
    ("#00FFFF", "[CYAN]"),
    ("#00FF00", "[GREEN]"),
    ("#FF00FF", "[MAGENTA]"),
    ("#FF0000", "[RED]"),
    ("#FFFFFF", "[WHITE]"),
    ("#FFFF00", "[YELLOW]"),
];

/// One section of an Excel format code from a `number:*-style`, without
/// its maps.
fn section(style: Node) -> Option<String> {
    let mut out = String::new();
    if let Some(c) = child(style, NS_STYLE, "text-properties")
        .and_then(|t| attr(t, NS_FO, "color"))
        .and_then(color)
        && let Some((_, name)) = COLOR_NAMES.iter().find(|(c2, _)| *c2 == c)
    {
        out.push_str(name);
    }
    let kind = style.tag_name().name();
    let long = |n: Node| attr(n, NS_NUMBER, "style") == Some("long");
    let elapsed = attr(style, NS_NUMBER, "truncate-on-overflow") == Some("false");
    let mut first_time = true;
    for n in style.children().filter(|n| n.is_element()) {
        let ns = n.tag_name().namespace();
        if ns == Some(NS_STYLE) {
            continue;
        }
        if ns != Some(NS_NUMBER) {
            return None;
        }
        let min_int = |n: Node| -> usize {
            attr(n, NS_NUMBER, "min-integer-digits")
                .and_then(|v| v.parse().ok())
                .unwrap_or(1)
        };
        match n.tag_name().name() {
            "number" => {
                let grouping = attr(n, NS_NUMBER, "grouping") == Some("true");
                out.push_str(&integer_part(min_int(n), grouping));
                out.push_str(&decimals(n));
            }
            "scientific-number" => {
                let interval: usize = attr(n, NS_NUMBER, "exponent-interval")
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(1);
                let int = min_int(n).max(1);
                let mut digits = "0".repeat(int);
                while digits.len() < interval {
                    digits.insert(0, '#');
                }
                out.push_str(&digits);
                out.push_str(&decimals(n));
                let exp: usize = attr(n, NS_NUMBER, "min-exponent-digits")
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(2);
                out.push_str("E+");
                out.push_str(&"0".repeat(exp.max(1)));
            }
            "fraction" => {
                let int = min_int(n);
                if int > 0 || attr(n, NS_NUMBER, "min-integer-digits").is_none() {
                    out.push_str("# ");
                }
                let num: usize = attr(n, NS_NUMBER, "min-numerator-digits")
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(1);
                out.push_str(&"?".repeat(num.max(1)));
                out.push('/');
                match attr(n, NS_NUMBER, "denominator-value") {
                    Some(d) => out.push_str(d),
                    None => {
                        let den: usize = attr(n, NS_NUMBER, "min-denominator-digits")
                            .and_then(|v| v.parse().ok())
                            .unwrap_or(1);
                        out.push_str(&"?".repeat(den.max(1)));
                    }
                }
            }
            "currency-symbol" => {
                let sym = n.text().unwrap_or_default().trim();
                quote(if sym.is_empty() { "$" } else { sym }, &mut out);
            }
            "text" => {
                let text = n.text().unwrap_or_default();
                match attr(n, NS_LOEXT, "blank-width-char") {
                    Some(b) if text.trim().is_empty() => {
                        if let Some(c) = b.chars().next() {
                            out.push('_');
                            out.push(c);
                        }
                    }
                    _ => quote(text, &mut out),
                }
            }
            "fill-character" => {
                out.push('*');
                out.push(n.text().unwrap_or(" ").chars().next().unwrap_or(' '));
            }
            "text-content" => out.push('@'),
            "day" => out.push_str(if long(n) { "dd" } else { "d" }),
            "month" => {
                let textual = attr(n, NS_NUMBER, "textual") == Some("true");
                out.push_str(match (textual, long(n)) {
                    (true, true) => "mmmm",
                    (true, false) => "mmm",
                    (false, true) => "mm",
                    (false, false) => "m",
                });
            }
            "year" => out.push_str(if long(n) { "yyyy" } else { "yy" }),
            "day-of-week" => out.push_str(if long(n) { "dddd" } else { "ddd" }),
            "hours" => {
                let h = if long(n) { "hh" } else { "h" };
                if elapsed && first_time {
                    out.push_str(&format!("[{h}]"));
                } else {
                    out.push_str(h);
                }
                first_time = false;
            }
            "minutes" => {
                let m = if long(n) { "mm" } else { "m" };
                if elapsed && first_time {
                    out.push_str(&format!("[{m}]"));
                } else {
                    out.push_str(m);
                }
                first_time = false;
            }
            "seconds" => {
                out.push_str(if long(n) { "ss" } else { "s" });
                let places: usize = attr(n, NS_NUMBER, "decimal-places")
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(0);
                if places > 0 {
                    out.push('.');
                    out.push_str(&"0".repeat(places));
                }
            }
            "am-pm" => out.push_str("AM/PM"),
            "boolean" => return None,
            _ => return None,
        }
    }
    if kind == "text-style" && !out.contains('@') {
        out.push('@');
    }
    Some(out)
}

/// The Excel format code of the number style `name`, with the sections of
/// its maps (`value()>=0`: positive;negative, `value()>0` and `value()<0`:
/// positive;negative;zero).
fn format_code(name: &str, numbers: &HashMap<String, Node>) -> Option<String> {
    let style = *numbers.get(name)?;
    let main = section(style)?;
    let maps: Vec<(String, String)> = style
        .children()
        .filter(|c| c.tag_name().name() == "map" && c.tag_name().namespace() == Some(NS_STYLE))
        .filter_map(|m| {
            Some((
                attr(m, NS_STYLE, "condition")?.replace(' ', ""),
                attr(m, NS_STYLE, "apply-style-name")?.to_owned(),
            ))
        })
        .collect();
    let mapped = |cond: &str| {
        maps.iter()
            .find(|(c, _)| c == cond)
            .and_then(|(_, s)| numbers.get(s).and_then(|n| section(*n)))
    };
    let code = match maps.len() {
        0 => main,
        1 => match mapped("value()>=0") {
            Some(pos) => format!("{pos};{main}"),
            None => main,
        },
        2 => match (mapped("value()>0"), mapped("value()<0")) {
            (Some(pos), Some(neg)) => format!("{pos};{neg};{main}"),
            _ => main,
        },
        _ => main,
    };
    (!code.is_empty()).then_some(code)
}

// ------------------------------------------------------------------ the package

/// Every cell style (named and automatic), number style and table, column
/// and row style of a package.
struct Styles<'a, 'i> {
    cells: HashMap<String, (Look, Option<String>)>,
    numbers: HashMap<String, Node<'a, 'i>>,
    columns: HashMap<String, f64>,
    /// Height in points and whether it is fixed (not "optimal").
    rows: HashMap<String, (f64, bool)>,
    /// Shown, tab colour.
    tables: HashMap<String, (bool, Option<String>)>,
    /// The `table-cell` default style.
    default: Look,
}

impl<'a, 'i> Styles<'a, 'i> {
    fn read(docs: &[&'a Document<'i>]) -> Self {
        let mut fonts = HashMap::new();
        for d in docs {
            for f in d
                .descendants()
                .filter(|n| n.has_tag_name((NS_STYLE, "font-face")))
            {
                if let (Some(name), Some(family)) =
                    (attr(f, NS_STYLE, "name"), attr(f, NS_SVG, "font-family"))
                {
                    fonts.insert(name.to_owned(), family.trim_matches('\'').to_owned());
                }
            }
        }
        let mut s = Styles {
            cells: HashMap::new(),
            numbers: HashMap::new(),
            columns: HashMap::new(),
            rows: HashMap::new(),
            tables: HashMap::new(),
            default: Look::default(),
        };
        for d in docs {
            for n in d.descendants().filter(|n| n.is_element()) {
                let tag = n.tag_name();
                if tag.namespace() == Some(NS_NUMBER) && tag.name().ends_with("-style") {
                    if let Some(name) = attr(n, NS_STYLE, "name") {
                        s.numbers.insert(name.to_owned(), n);
                    }
                    continue;
                }
                if tag.namespace() != Some(NS_STYLE) {
                    continue;
                }
                if tag.name() == "default-style"
                    && attr(n, NS_STYLE, "family") == Some("table-cell")
                {
                    s.default = read_look(n, &fonts);
                    continue;
                }
                if tag.name() != "style" {
                    continue;
                }
                let Some(name) = attr(n, NS_STYLE, "name") else {
                    continue;
                };
                match attr(n, NS_STYLE, "family") {
                    Some("table-cell") => {
                        let parent = attr(n, NS_STYLE, "parent-style-name").map(str::to_owned);
                        s.cells
                            .insert(name.to_owned(), (read_look(n, &fonts), parent));
                    }
                    Some("table-column") => {
                        if let Some(w) = child(n, NS_STYLE, "table-column-properties")
                            .and_then(|p| attr(p, NS_STYLE, "column-width"))
                            .and_then(points)
                        {
                            s.columns.insert(name.to_owned(), w);
                        }
                    }
                    Some("table-row") => {
                        if let Some(p) = child(n, NS_STYLE, "table-row-properties")
                            && let Some(h) = attr(p, NS_STYLE, "row-height").and_then(points)
                        {
                            let fixed = attr(p, NS_STYLE, "use-optimal-row-height") != Some("true");
                            s.rows.insert(name.to_owned(), (h, fixed));
                        }
                    }
                    Some("table") => {
                        let p = child(n, NS_STYLE, "table-properties");
                        let shown = p.and_then(|p| attr(p, NS_TABLE, "display")) != Some("false");
                        let tab = p
                            .and_then(|p| attr(p, NS_TABLE, "tab-color"))
                            .and_then(color);
                        s.tables.insert(name.to_owned(), (shown, tab));
                    }
                    _ => {}
                }
            }
        }
        s
    }

    /// A cell style with its parents applied, on the default.
    fn look(&self, name: &str) -> Option<Look> {
        let mut chain = Vec::new();
        let mut at = Some(name.to_owned());
        while let Some(n) = at {
            if chain.len() > 20 {
                break;
            }
            let (look, parent) = self.cells.get(&n)?;
            chain.push(look);
            at = parent.clone();
        }
        Some(
            chain
                .into_iter()
                .rev()
                .fold(self.default.clone(), |acc, l| acc.with(l)),
        )
    }
}

/// Excel column width (characters of a 7-pixel digit) of `pt` points.
fn width_chars(pt: f64) -> f64 {
    let px = pt * 96.0 / 72.0;
    (((px - 5.0) / 7.0).max(0.0) * 256.0).round() / 256.0
}

/// IronCalc style indices for cell styles, made when first used.
struct StyleMap<'s, 'a, 'i> {
    styles: &'s Styles<'a, 'i>,
    map: HashMap<String, i32>,
    base: Font,
}

impl StyleMap<'_, '_, '_> {
    fn get(&mut self, model: &mut Model<'_>, name: &str) -> i32 {
        if let Some(&s) = self.map.get(name) {
            return s;
        }
        let s = match self.styles.look(name) {
            Some(l) => {
                let style = self.convert(&l);
                if model.workbook.styles.get_style(0).is_ok_and(|d| d == style) {
                    0
                } else {
                    let st = &mut model.workbook.styles;
                    st.get_style_index(&style)
                        .unwrap_or_else(|| st.create_new_style(&style))
                }
            }
            None => 0,
        };
        self.map.insert(name.to_owned(), s);
        s
    }

    fn convert(&self, l: &Look) -> Style {
        let base = &self.base;
        let font = Font {
            strike: l.strike.unwrap_or(false),
            u: l.underline.unwrap_or(false),
            b: l.bold.unwrap_or(false),
            i: l.italic.unwrap_or(false),
            sz: l.size.map_or(base.sz, |s| (s.round() as i32).clamp(1, 409)),
            color: match &l.color {
                Some(Some(c)) => Color::Rgb(c.clone()),
                _ => base.color.clone(),
            },
            name: l.font.clone().unwrap_or_else(|| base.name.clone()),
            family: base.family,
            scheme: FontScheme::None,
        };
        let alignment = Alignment {
            horizontal: l.halign.clone().unwrap_or(HorizontalAlignment::General),
            vertical: l.valign.clone().unwrap_or(VerticalAlignment::Bottom),
            wrap_text: l.wrap.unwrap_or(false),
            // Excel indents left, right and general (left) text.
            indent: if matches!(
                l.halign,
                None | Some(
                    HorizontalAlignment::Left
                        | HorizontalAlignment::General
                        | HorizontalAlignment::Right
                )
            ) {
                l.indent.unwrap_or(0)
            } else {
                0
            },
            text_rotation: l.rotation.unwrap_or(0),
            shrink_to_fit: l.shrink.unwrap_or(false),
        };
        let alignment = (alignment != Alignment::default()).then_some(alignment);
        let b = |i: usize| l.border[i].clone().flatten();
        let num_fmt = l
            .data_style
            .as_deref()
            .and_then(|d| format_code(d, &self.styles.numbers))
            .unwrap_or_else(|| "general".to_owned());
        Style {
            alignment,
            num_fmt,
            fill: Fill {
                color: match &l.fill {
                    Some(Some(c)) => Color::Rgb(c.clone()),
                    _ => Color::None,
                },
            },
            font,
            border: Border {
                left: b(0),
                right: b(1),
                top: b(2),
                bottom: b(3),
                diagonal: None,
                diagonal_down: false,
                diagonal_up: false,
            },
            quote_prefix: false,
            protection: l
                .protection
                .filter(|&(locked, hidden)| !locked || hidden)
                .map(|(locked, hidden)| CellProtection { locked, hidden }),
        }
    }
}

/// View settings of one table (`settings.xml`).
#[derive(Debug, Default, PartialEq)]
struct View {
    /// Frozen columns and rows.
    frozen: (u32, u32),
    grid: Option<bool>,
}

/// The tables' view settings and the active table's name.
fn read_settings(xml: &str) -> (HashMap<String, View>, Option<String>) {
    let mut views = HashMap::new();
    let mut active = None;
    let Ok(doc) = Document::parse(xml) else {
        return (views, active);
    };
    let item = |n: Node, name: &str| {
        n.children()
            .find(|c| {
                c.has_tag_name((NS_CONFIG, "config-item"))
                    && attr(*c, NS_CONFIG, "name") == Some(name)
            })
            .and_then(|c| c.text())
            .map(str::to_owned)
    };
    let Some(view) = doc.descendants().find(|n| {
        n.has_tag_name((NS_CONFIG, "config-item-map-entry"))
            && n.parent().is_some_and(|p| {
                p.has_tag_name((NS_CONFIG, "config-item-map-indexed"))
                    && attr(p, NS_CONFIG, "name") == Some("Views")
            })
    }) else {
        return (views, active);
    };
    active = item(view, "ActiveTable");
    let global_grid = item(view, "ShowGrid").map(|v| v == "true");
    if let Some(tables) = view.children().find(|c| {
        c.has_tag_name((NS_CONFIG, "config-item-map-named"))
            && attr(*c, NS_CONFIG, "name") == Some("Tables")
    }) {
        for t in tables
            .children()
            .filter(|c| c.has_tag_name((NS_CONFIG, "config-item-map-entry")))
        {
            let Some(name) = attr(t, NS_CONFIG, "name") else {
                continue;
            };
            let num = |k: &str| item(t, k).and_then(|v| v.parse::<u32>().ok()).unwrap_or(0);
            // Mode 2 is frozen; 1 is a plain split, not kept.
            let cols = if num("HorizontalSplitMode") == 2 {
                num("HorizontalSplitPosition")
            } else {
                0
            };
            let rows = if num("VerticalSplitMode") == 2 {
                num("VerticalSplitPosition")
            } else {
                0
            };
            views.insert(
                name.to_owned(),
                View {
                    frozen: (cols, rows),
                    grid: item(t, "ShowGrid").map(|v| v == "true").or(global_grid),
                },
            );
        }
    }
    (views, active)
}

fn part(path: &Path, name: &str) -> Result<Option<String>> {
    let file = std::fs::File::open(path)?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| anyhow!("{e}"))?;
    let Ok(mut entry) = zip.by_name(name) else {
        return Ok(None);
    };
    let mut s = String::new();
    entry.read_to_string(&mut s)?;
    Ok(Some(s))
}

/// What applying an `.ods`'s formatting brought.
#[derive(Default)]
pub(crate) struct Applied {
    /// Values changed (covered cells of merges were cleared).
    pub cleared: bool,
    pub active: Option<u32>,
}

/// Read the formatting of the `.ods` at `path` and apply it to `model`,
/// whose sheets calamine filled in `names` order.
pub(crate) fn apply(path: &Path, model: &mut Model<'_>, names: &[String]) -> Result<Applied> {
    let content = part(path, "content.xml")?.ok_or_else(|| anyhow!("no content.xml"))?;
    let styles_xml = part(path, "styles.xml")?.unwrap_or_default();
    let settings = part(path, "settings.xml")?.unwrap_or_default();
    let opts = || roxmltree::ParsingOptions {
        nodes_limit: u32::MAX,
        ..roxmltree::ParsingOptions::default()
    };
    let content = Document::parse_with_options(&content, opts()).map_err(|e| anyhow!("{e}"))?;
    let styles_doc = Document::parse_with_options(&styles_xml, opts()).ok();
    let mut docs = Vec::new();
    if let Some(d) = &styles_doc {
        docs.push(d);
    }
    docs.push(&content);
    let styles = Styles::read(&docs);
    let (views, active) = read_settings(&settings);

    // The default font: the workbook's, so unformatted cells look as in
    // the file.
    let mut base = model
        .workbook
        .styles
        .fonts
        .first()
        .cloned()
        .unwrap_or_default();
    let default = styles
        .look("Default")
        .unwrap_or_else(|| styles.default.clone());
    if let Some(name) = &default.font {
        base.name = name.clone();
    }
    if let Some(sz) = default.size {
        base.sz = (sz.round() as i32).clamp(1, 409);
    }
    if let Some(slot) = model.workbook.styles.fonts.first_mut() {
        *slot = base.clone();
    }
    let mut map = StyleMap {
        styles: &styles,
        map: HashMap::new(),
        base,
    };
    let mut applied = Applied {
        active: active
            .and_then(|a| names.iter().position(|n| *n == a))
            .map(|i| i as u32),
        ..Applied::default()
    };
    let body = content
        .descendants()
        .find(|n| n.has_tag_name((NS_OFFICE, "spreadsheet")));
    let tables: Vec<Node> = body
        .map(|b| {
            b.children()
                .filter(|n| n.has_tag_name((NS_TABLE, "table")))
                .collect()
        })
        .unwrap_or_default();
    for table in tables {
        let Some(name) = attr(table, NS_TABLE, "name") else {
            continue;
        };
        let Some(sheet) = names.iter().position(|n| n == name) else {
            continue;
        };
        apply_table(
            table,
            sheet as u32,
            model,
            &mut map,
            views.get(name),
            &mut applied,
        );
    }
    Ok(applied)
}

/// The rows (or columns) of `table` in order, wherever they are grouped.
fn lines<'a, 'i>(table: Node<'a, 'i>, tag: &str) -> Vec<Node<'a, 'i>> {
    table
        .descendants()
        .filter(|n| n.has_tag_name((NS_TABLE, tag)))
        .filter(|n| {
            n.ancestors()
                .skip(1)
                .find(|a| a.has_tag_name((NS_TABLE, "table")))
                == Some(table)
        })
        .collect()
}

fn repeated(n: Node, what: &str) -> u32 {
    attr(n, NS_TABLE, what)
        .and_then(|v| v.parse::<u32>().ok())
        .unwrap_or(1)
        .max(1)
}

fn apply_table(
    table: Node,
    sheet: u32,
    model: &mut Model<'_>,
    map: &mut StyleMap,
    view: Option<&View>,
    applied: &mut Applied,
) {
    let styles = map.styles;
    // Columns: widths, visibility, default cell styles.
    let mut cols: Vec<Col> = Vec::new();
    let mut col_defaults: Vec<(u32, u32, String)> = Vec::new();
    let mut c = 0u32;
    for col in lines(table, "table-column") {
        let n = repeated(col, "number-columns-repeated");
        if c >= MAX_COLS {
            break;
        }
        let last = c.saturating_add(n - 1).min(MAX_COLS - 1);
        if let Some(d) = attr(col, NS_TABLE, "default-cell-style-name")
            && d != "Default"
        {
            col_defaults.push((c, last, d.to_owned()));
        }
        let width = attr(col, NS_TABLE, "style-name").and_then(|s| styles.columns.get(s));
        let hidden = matches!(
            attr(col, NS_TABLE, "visibility"),
            Some("collapse" | "filter")
        );
        if (width.is_some() || hidden) && n <= MAX_LINES {
            cols.push(Col {
                min: c as i32 + 1,
                max: last as i32 + 1,
                width: width.map_or(8.43, |w| width_chars(*w)),
                custom_width: true,
                hidden,
                style: None,
                outline_level: 0,
                collapsed: false,
            });
        }
        c = c.saturating_add(n);
    }
    for (first, last, name) in &col_defaults {
        let s = map.get(model, name);
        if s == 0 {
            continue;
        }
        for col in cols.iter_mut() {
            let (lo, hi) = (col.min as u32 - 1, col.max as u32 - 1);
            if lo >= *first && hi <= *last {
                col.style = Some(s);
            }
        }
    }
    let col_default = |col: u32| {
        col_defaults
            .iter()
            .find(|(a, b, _)| (*a..=*b).contains(&col))
            .map(|(_, _, n)| n.as_str())
    };

    let mut rows: Vec<Row> = Vec::new();
    let mut merges: Vec<MergedCell> = Vec::new();
    let mut r = 0u32;
    for row in lines(table, "table-row") {
        if r >= MAX_ROWS {
            break;
        }
        let n = repeated(row, "number-rows-repeated");
        let height = attr(row, NS_TABLE, "style-name").and_then(|s| styles.rows.get(s));
        let hidden = matches!(
            attr(row, NS_TABLE, "visibility"),
            Some("collapse" | "filter")
        );
        let row_default = attr(row, NS_TABLE, "default-cell-style-name");
        if (height.is_some_and(|h| h.1) || hidden) && n <= MAX_LINES {
            for i in 0..n {
                let at = r + i;
                if at >= MAX_ROWS {
                    break;
                }
                rows.push(Row {
                    r: at as i32 + 1,
                    height: height.map_or(crate::rows::DEFAULT_ROW_POINTS, |h| h.0),
                    custom_format: false,
                    custom_height: height.is_some_and(|h| h.1),
                    s: 0,
                    hidden,
                    outline_level: 0,
                    collapsed: false,
                });
            }
        }
        let mut c = 0u32;
        for cell in row.children().filter(|n| {
            n.has_tag_name((NS_TABLE, "table-cell"))
                || n.has_tag_name((NS_TABLE, "covered-table-cell"))
        }) {
            let m = repeated(cell, "number-columns-repeated");
            if c >= MAX_COLS {
                break;
            }
            let style = attr(cell, NS_TABLE, "style-name")
                .or(row_default)
                .or_else(|| col_default(c));
            if let Some(name) = style
                && name != "Default"
                && u64::from(n) * u64::from(m) <= MAX_RUN
            {
                let s = map.get(model, name);
                if s != 0 {
                    for dr in 0..n {
                        for dc in 0..m {
                            let (rr, cc) = (r + dr, c + dc);
                            if rr >= MAX_ROWS || cc >= MAX_COLS {
                                continue;
                            }
                            set_style(model, sheet, rr, cc, s);
                        }
                    }
                }
            }
            let spans = (
                repeated(cell, "number-columns-spanned"),
                repeated(cell, "number-rows-spanned"),
            );
            if cell.has_tag_name((NS_TABLE, "table-cell")) && spans != (1, 1) && n == 1 {
                let w = spans.0.min(MAX_COLS - c);
                let h = spans.1.min(MAX_ROWS - r);
                let mc = MergedCell {
                    row: r as i32 + 1,
                    column: c as i32 + 1,
                    width: w as i32,
                    height: h as i32,
                };
                if (w, h) != (1, 1)
                    && !merges
                        .iter()
                        .any(|o| o.intersects(mc.row, mc.column, mc.width, mc.height))
                {
                    merges.push(mc);
                }
            }
            c = c.saturating_add(m);
        }
        r = r.saturating_add(n);
    }

    let Ok(ws) = model.workbook.worksheet_mut(sheet) else {
        return;
    };
    cols.sort_by_key(|c| c.min);
    ws.cols = cols;
    rows.sort_by_key(|r| r.r);
    rows.dedup_by_key(|r| r.r);
    ws.rows = rows;
    for m in &merges {
        for rr in m.row..=m.last_row() {
            for cc in m.column..=m.last_column() {
                if (rr, cc) != (m.row, m.column)
                    && ws.cell(rr, cc).is_some_and(|cell| {
                        !matches!(*cell, ironcalc_base::types::Cell::EmptyCell { .. })
                    })
                {
                    applied.cleared |= ws.cell_clear_contents(rr, cc).is_ok();
                }
            }
        }
    }
    ws.merged_cells = merges;
    if let Some((shown, tab)) =
        attr(table, NS_TABLE, "style-name").and_then(|s| styles.tables.get(s))
    {
        if !shown {
            ws.state = SheetState::Hidden;
        }
        if let Some(t) = tab {
            ws.color = Color::Rgb(t.clone());
        }
    }
    if let Some(v) = view {
        ws.frozen_columns = v.frozen.0.min(MAX_COLS - 1) as i32;
        ws.frozen_rows = v.frozen.1.min(MAX_ROWS - 1) as i32;
        if let Some(g) = v.grid {
            ws.show_grid_lines = g;
        }
    }
}

/// Give a cell a style, keeping a date or time format the import gave it
/// when the style has no number format of its own.
fn set_style(model: &mut Model<'_>, sheet: u32, row: u32, col: u32, s: i32) {
    let (r, c) = (row as i32 + 1, col as i32 + 1);
    let mut s = s;
    let styles = &model.workbook.styles;
    let new_general = styles
        .get_style(s)
        .is_ok_and(|st| st.num_fmt.eq_ignore_ascii_case("general"));
    if new_general
        && let Ok(old) = model.get_style_for_cell(sheet, r, c)
        && !old.num_fmt.eq_ignore_ascii_case("general")
        && let Ok(mut st) = model.workbook.styles.get_style(s)
    {
        st.num_fmt = old.num_fmt;
        let styles = &mut model.workbook.styles;
        s = styles
            .get_style_index(&st)
            .unwrap_or_else(|| styles.create_new_style(&st));
    }
    if let Ok(ws) = model.workbook.worksheet_mut(sheet) {
        let _ = ws.set_cell_style(r, c, s);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn code(xml: &str, name: &str) -> Option<String> {
        let full = format!(
            "<r xmlns:number=\"{NS_NUMBER}\" xmlns:style=\"{NS_STYLE}\" xmlns:fo=\"{NS_FO}\" xmlns:loext=\"{NS_LOEXT}\">{xml}</r>"
        );
        let doc = Document::parse(&full).unwrap();
        let numbers: HashMap<String, Node> = doc
            .descendants()
            .filter(|n| {
                n.tag_name().namespace() == Some(NS_NUMBER)
                    && n.tag_name().name().ends_with("-style")
            })
            .map(|n| (attr(n, NS_STYLE, "name").unwrap().to_owned(), n))
            .collect();
        format_code(name, &numbers)
    }

    #[test]
    fn number_formats() {
        let xml = r##"<number:number-style style:name="P0"><number:number number:decimal-places="2" number:min-decimal-places="2" number:min-integer-digits="1" number:grouping="true"/></number:number-style>
<number:number-style style:name="N"><style:text-properties fo:color="#ff0000"/><number:text>-</number:text><number:number number:decimal-places="2" number:min-decimal-places="2" number:min-integer-digits="1" number:grouping="true"/><style:map style:condition="value()&gt;=0" style:apply-style-name="P0"/></number:number-style>
<number:number-style style:name="Rp"><number:text>Rp </number:text><number:number number:decimal-places="0" number:min-integer-digits="1" number:grouping="true"/></number:number-style>
<number:percentage-style style:name="Pct"><number:number number:decimal-places="1" number:min-decimal-places="1" number:min-integer-digits="1"/><number:text>%</number:text></number:percentage-style>
<number:date-style style:name="D"><number:day number:style="long"/><number:text>/</number:text><number:month number:style="long"/><number:text>/</number:text><number:year number:style="long"/></number:date-style>
<number:date-style style:name="D2"><number:day/><number:text>-</number:text><number:month number:textual="true"/><number:text>-</number:text><number:year/></number:date-style>
<number:time-style style:name="T" number:truncate-on-overflow="false"><number:hours/><number:text>:</number:text><number:minutes number:style="long"/><number:text>:</number:text><number:seconds number:style="long"/></number:time-style>
<number:time-style style:name="T2"><number:hours/><number:text>:</number:text><number:minutes number:style="long"/><number:text> </number:text><number:am-pm/></number:time-style>
<number:number-style style:name="E"><number:scientific-number number:decimal-places="3" number:min-decimal-places="3" number:min-integer-digits="1" number:min-exponent-digits="2"/></number:number-style>
<number:text-style style:name="Txt"><number:text-content/></number:text-style>
<number:number-style style:name="Pad"><number:number number:decimal-places="0" number:min-integer-digits="1" number:grouping="true"/><number:text loext:blank-width-char=")"> </number:text></number:number-style>"##;
        assert_eq!(code(xml, "N").as_deref(), Some("#,##0.00;[RED]-#,##0.00"));
        assert_eq!(code(xml, "Rp").as_deref(), Some("\"Rp \"#,##0"));
        assert_eq!(code(xml, "Pct").as_deref(), Some("0.0%"));
        assert_eq!(code(xml, "D").as_deref(), Some("dd/mm/yyyy"));
        assert_eq!(code(xml, "D2").as_deref(), Some("d-mmm-yy"));
        assert_eq!(code(xml, "T").as_deref(), Some("[h]:mm:ss"));
        assert_eq!(code(xml, "T2").as_deref(), Some("h:mm AM/PM"));
        assert_eq!(code(xml, "E").as_deref(), Some("0.000E+00"));
        assert_eq!(code(xml, "Txt").as_deref(), Some("@"));
        assert_eq!(code(xml, "Pad").as_deref(), Some("#,##0_)"));
        assert_eq!(code(xml, "missing"), None);
    }

    #[test]
    fn lengths_borders_widths() {
        assert_eq!(points("1in"), Some(72.0));
        assert!((points("2.54cm").unwrap() - 72.0).abs() < 1e-9);
        assert_eq!(points("bad"), None);
        let b = border("0.74pt solid #1f4e78").unwrap();
        assert_eq!(b.style, BorderStyle::Thin);
        assert_eq!(b.color, Color::Rgb("#1F4E78".into()));
        assert_eq!(
            border("1.76pt solid #000000").unwrap().style,
            BorderStyle::Medium
        );
        assert_eq!(
            border("2.5pt solid #000000").unwrap().style,
            BorderStyle::Thick
        );
        assert_eq!(
            border("1.1pt double #000000").unwrap().style,
            BorderStyle::Double
        );
        assert_eq!(
            border("0.74pt dashed #000000").unwrap().style,
            BorderStyle::Dashed
        );
        assert!(border("none").is_none());
        // 64 px: Excel's 8.43 characters.
        assert!((width_chars(48.0) - 8.43).abs() < 0.01);
    }

    #[test]
    fn frozen_panes_from_settings() {
        let xml = format!(
            r#"<office:document-settings xmlns:office="{NS_OFFICE}" xmlns:config="{NS_CONFIG}"><office:settings><config:config-item-set config:name="ooo:view-settings"><config:config-item-map-indexed config:name="Views"><config:config-item-map-entry><config:config-item config:name="ActiveTable" config:type="string">Two</config:config-item><config:config-item config:name="ShowGrid" config:type="boolean">true</config:config-item><config:config-item-map-named config:name="Tables"><config:config-item-map-entry config:name="One"><config:config-item config:name="HorizontalSplitMode" config:type="short">2</config:config-item><config:config-item config:name="VerticalSplitMode" config:type="short">2</config:config-item><config:config-item config:name="HorizontalSplitPosition" config:type="int">1</config:config-item><config:config-item config:name="VerticalSplitPosition" config:type="int">2</config:config-item></config:config-item-map-entry><config:config-item-map-entry config:name="Two"><config:config-item config:name="ShowGrid" config:type="boolean">false</config:config-item><config:config-item config:name="HorizontalSplitMode" config:type="short">1</config:config-item><config:config-item config:name="HorizontalSplitPosition" config:type="int">400</config:config-item></config:config-item-map-entry></config:config-item-map-named></config:config-item-map-entry></config:config-item-map-indexed></config:config-item-set></office:settings></office:document-settings>"#
        );
        let (views, active) = read_settings(&xml);
        assert_eq!(active.as_deref(), Some("Two"));
        assert_eq!(
            views["One"],
            View {
                frozen: (1, 2),
                grid: Some(true)
            }
        );
        assert_eq!(
            views["Two"],
            View {
                frozen: (0, 0),
                grid: Some(false)
            }
        );
        assert_eq!(read_settings("not xml").0.len(), 0);
    }
}
