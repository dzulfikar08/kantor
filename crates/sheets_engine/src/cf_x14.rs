//! Excel 2010 extras of conditional formatting that IronCalc's model doesn't
//! hold: a data bar's minimum and maximum lengths, its borders, negative
//! colours and axis (`<x14:dataBar>`), and custom icon sets (x14-only
//! `iconSet` rules, which IronCalc can't write back).
//!
//! They are read from the opened file with the model rule they belong to
//! (same range, same kind). While that rule is unchanged, the grid and the
//! PDF draw its bar with them, and a save that writes the sheet's rules from
//! IronCalc's model (a sheet whose rules were edited, `preserve`) puts the
//! file's x14 markup back for it ([`patch_sheet`]). An edited rule is
//! Kantor's own (Excel 2010 defaults: lengths 0 to 100 %, no border).

use std::collections::HashMap;

use ironcalc_base::cf_types::{CfRule, ConditionalFormatting};
use ironcalc_base::types::Color;
use roxmltree::{Document, Node};

const NS_X14: &str = "http://schemas.microsoft.com/office/spreadsheetml/2009/9/main";
const CF_EXTENSION: &str = "{78C0D931-6437-407d-A8EE-F0AAD7539E65}";
const NS_XM: &str = "http://schemas.microsoft.com/office/excel/2006/main";

/// Where a data bar's axis goes (`axisPosition`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Axis {
    /// At zero when the range has negative and positive values.
    #[default]
    Automatic,
    /// In the middle of the cell.
    Middle,
    /// None: negative bars grow left to right like positive ones.
    None,
}

/// Which way a data bar grows (`direction`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Direction {
    #[default]
    Context,
    LeftToRight,
    RightToLeft,
}

/// How a data bar is drawn beyond its colours.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct BarLook {
    /// Shortest and longest bar, fractions of the cell's width.
    pub min_len: f32,
    pub max_len: f32,
    pub border: Option<Color>,
    /// Negative bars' border (`None`: the positive one).
    pub negative_border: Option<Color>,
    /// Negative bars' fill, when the file gives one.
    pub negative_fill: Option<Color>,
    pub axis: Axis,
    pub axis_color: Option<Color>,
    pub direction: Direction,
}

impl BarLook {
    /// A bar Kantor (or IronCalc) wrote: Excel 2010's defaults.
    pub(crate) const KANTOR: BarLook = BarLook {
        min_len: 0.0,
        max_len: 1.0,
        border: None,
        negative_border: None,
        negative_fill: None,
        axis: Axis::Automatic,
        axis_color: None,
        direction: Direction::Context,
    };

    /// Its `<x14:dataBar>` element with `gradient` and the ends `min` and
    /// `max` (IronCalc's form, `None`: automatic), as Excel 2010 writes it.
    pub(crate) fn x14_xml(
        &self,
        gradient: bool,
        min: Option<&ironcalc_base::cf_types::Cfvo>,
        max: Option<&ironcalc_base::cf_types::Cfvo>,
        negative_fill: &Color,
    ) -> String {
        let pct = |f: f32| (f * 100.0).round().clamp(0.0, 100.0) as u32;
        let mut x = format!(
            "<x14:dataBar xmlns:xm=\"{NS_XM}\" minLength=\"{}\" maxLength=\"{}\"",
            pct(self.min_len),
            pct(self.max_len)
        );
        if !gradient {
            x.push_str(" gradient=\"0\"");
        }
        if self.border.is_some() {
            x.push_str(" border=\"1\"");
        }
        match self.direction {
            Direction::Context => {}
            Direction::LeftToRight => x.push_str(" direction=\"leftToRight\""),
            Direction::RightToLeft => x.push_str(" direction=\"rightToLeft\""),
        }
        if self.border.is_some() && self.negative_border.is_some() {
            x.push_str(" negativeBarBorderColorSameAsPositive=\"0\"");
        }
        match self.axis {
            Axis::Automatic => {}
            Axis::Middle => x.push_str(" axisPosition=\"middle\""),
            Axis::None => x.push_str(" axisPosition=\"none\""),
        }
        x.push('>');
        let cfvo = |c: Option<&ironcalc_base::cf_types::Cfvo>, auto: &str| {
            use ironcalc_base::cf_types::Cfvo;
            let f = |t: &str, v: String| {
                format!(
                    "<x14:cfvo type=\"{t}\"><xm:f>{}</xm:f></x14:cfvo>",
                    escape(&v)
                )
            };
            match c {
                None => format!("<x14:cfvo type=\"{auto}\"/>"),
                Some(Cfvo::Min) => "<x14:cfvo type=\"min\"/>".into(),
                Some(Cfvo::Max) => "<x14:cfvo type=\"max\"/>".into(),
                Some(Cfvo::Number(n)) => f("num", n.to_string()),
                Some(Cfvo::Percent(n)) => f("percent", n.to_string()),
                Some(Cfvo::Percentile(n)) => f("percentile", n.to_string()),
                Some(Cfvo::Formula(s)) => f("formula", s.clone()),
            }
        };
        x.push_str(&cfvo(min, "autoMin"));
        x.push_str(&cfvo(max, "autoMax"));
        if let Some(c) = &self.border {
            x.push_str(&color_xml("borderColor", c));
        }
        x.push_str(&color_xml("negativeFillColor", negative_fill));
        if self.border.is_some()
            && let Some(c) = &self.negative_border
        {
            x.push_str(&color_xml("negativeBorderColor", c));
        }
        x.push_str(&color_xml(
            "axisColor",
            self.axis_color
                .as_ref()
                .unwrap_or(&Color::Rgb("#000000".into())),
        ));
        x.push_str("</x14:dataBar>");
        x
    }
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// `<x14:NAME rgb=… />` (or theme and tint).
fn color_xml(name: &str, c: &Color) -> String {
    match c {
        Color::Rgb(s) => format!(
            "<x14:{name} rgb=\"FF{}\"/>",
            s.trim_start_matches('#').to_uppercase()
        ),
        Color::Theme(t, tint) if *tint == 0.0 => format!("<x14:{name} theme=\"{t}\"/>"),
        Color::Theme(t, tint) => format!("<x14:{name} theme=\"{t}\" tint=\"{tint}\"/>"),
        Color::None => format!("<x14:{name} rgb=\"FF000000\"/>"),
    }
}

/// An icon set rule as Excel 2010 writes a custom one (icons picked one by
/// one): the whole `<x14:conditionalFormatting>` for `range`.
pub(crate) fn custom_icons_xml(
    range: &str,
    set: &str,
    show_value: bool,
    steps: &[(ironcalc_base::cf_types::Cfvo, bool, String, u8)],
) -> String {
    use ironcalc_base::cf_types::Cfvo;
    let mut x = format!(
        "<x14:conditionalFormatting xmlns:xm=\"{NS_XM}\"><x14:cfRule type=\"iconSet\" priority=\"1\" id=\"{{{}}}\"><x14:iconSet iconSet=\"{set}\"",
        guid(range, steps.len())
    );
    if !show_value {
        x.push_str(" showValue=\"0\"");
    }
    x.push_str(" custom=\"1\">");
    for (cfvo, gte, _, _) in steps {
        let (t, v) = match cfvo {
            Cfvo::Min => ("min", None),
            Cfvo::Max => ("max", None),
            Cfvo::Number(n) => ("num", Some(n.to_string())),
            Cfvo::Percent(n) => ("percent", Some(n.to_string())),
            Cfvo::Percentile(n) => ("percentile", Some(n.to_string())),
            Cfvo::Formula(f) => ("formula", Some(f.clone())),
        };
        let gte = if *gte { "" } else { " gte=\"0\"" };
        match v {
            Some(v) => x.push_str(&format!(
                "<x14:cfvo type=\"{t}\"{gte}><xm:f>{}</xm:f></x14:cfvo>",
                escape(&v)
            )),
            None => x.push_str(&format!("<x14:cfvo type=\"{t}\"{gte}/>")),
        }
    }
    for (_, _, icon_set, id) in steps {
        x.push_str(&format!(
            "<x14:cfIcon iconSet=\"{icon_set}\" iconId=\"{id}\"/>"
        ));
    }
    x.push_str(&format!(
        "</x14:iconSet></x14:cfRule><xm:sqref>{range}</xm:sqref></x14:conditionalFormatting>"
    ));
    x
}

/// A stable GUID for a rule Kantor writes.
fn guid(range: &str, n: usize) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in range.bytes().chain(n.to_le_bytes()) {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x100000001b3);
    }
    format!(
        "{:08X}-{:04X}-4{:03X}-8{:03X}-{:012X}",
        (h >> 32) as u32,
        (h >> 16) as u16,
        (h & 0xFFF) as u16,
        ((h >> 20) & 0xFFF) as u16,
        h & 0xFFFF_FFFF_FFFF
    )
}

/// One x14 rule of the opened file.
#[derive(Debug, Clone)]
pub(crate) struct X14Rule {
    pub sheet_id: u32,
    pub range: String,
    pub data_bar: bool,
    /// The `<x14:conditionalFormatting>` element (prefixes `x14`, `xm`).
    pub xml: String,
    /// Its `<x14:dataBar>` element, for a data bar.
    pub bar_xml: Option<String>,
    /// The model rule it belongs to, as the file was opened.
    pub rule: Option<CfRule>,
}

/// The file's x14 rules and data bar looks.
#[derive(Debug, Clone, Default)]
pub(crate) struct CfX14 {
    pub rules: Vec<X14Rule>,
    /// Data bars of the file: `(sheetId, range)` -> (model rule at open,
    /// look).
    pub bars: HashMap<(u32, String), (Option<CfRule>, BarLook)>,
}

fn color_of(n: Node) -> Option<Color> {
    if let Some(rgb) = n.attribute("rgb") {
        return Some(Color::Rgb(format!(
            "#{}",
            rgb.get(rgb.len().saturating_sub(6)..)?
        )));
    }
    let slot = n.attribute("theme")?.parse().ok()?;
    let tint = n
        .attribute("tint")
        .and_then(|t| t.parse().ok())
        .unwrap_or(0.0);
    Some(Color::Theme(slot, tint))
}

fn child<'a, 'i>(n: Node<'a, 'i>, name: &str) -> Option<Node<'a, 'i>> {
    n.children()
        .find(|c| c.is_element() && c.tag_name().name() == name)
}

fn flag(n: Node, name: &str, default: bool) -> bool {
    match n.attribute(name) {
        Some("1" | "true") => true,
        Some("0" | "false") => false,
        _ => default,
    }
}

fn length(n: Node, name: &str, default: f32) -> f32 {
    n.attribute(name)
        .and_then(|v| v.parse::<f32>().ok())
        .map_or(default, |v| (v / 100.0).clamp(0.0, 1.0))
}

/// An element's text as a standalone fragment: only the `x14` and `xm`
/// prefixes, the `xm` one declared on it.
fn fragment(n: Node, src: &str) -> Option<String> {
    let text = &src[n.range()];
    let prefixes_ok = n.descendants().filter(|d| d.is_element()).all(|d| {
        matches!(d.tag_name().namespace(), Some(NS_X14 | NS_XM))
            && d.attributes()
                .all(|a| a.namespace().is_none_or(|ns| ns == NS_X14 || ns == NS_XM))
    });
    if !prefixes_ok || !text.starts_with("<x14:") {
        return None;
    }
    let uses_xm = text.contains("<xm:");
    if uses_xm && !text[..text.find('>')?].contains("xmlns:xm=") {
        let at = text.find(|c: char| c.is_whitespace() || c == '>' || c == '/')?;
        return Some(format!(
            "{} xmlns:xm=\"{NS_XM}\"{}",
            &text[..at],
            &text[at..]
        ));
    }
    Some(text.to_owned())
}

/// The x14 rules and data bar looks of each worksheet (`sheetId`, XML
/// without its cells).
pub(crate) fn read(skeletons: &Result<Vec<(u32, String)>, String>) -> CfX14 {
    let mut out = CfX14::default();
    let Ok(list) = skeletons else {
        return out;
    };
    for (sheet_id, src) in list {
        let Ok(doc) = Document::parse(src) else {
            continue;
        };
        let root = doc.root_element();
        // Main data bars: their lengths (Excel 2007: 10 % to 90 %), and the
        // ids of their x14 parts.
        let mut ids: HashMap<String, String> = HashMap::new();
        for cf in root
            .children()
            .filter(|n| n.is_element() && n.tag_name().name() == "conditionalFormatting")
        {
            let Some(range) = cf.attribute("sqref") else {
                continue;
            };
            for rule in cf
                .children()
                .filter(|n| n.is_element() && n.attribute("type") == Some("dataBar"))
            {
                let Some(bar) = child(rule, "dataBar") else {
                    continue;
                };
                let look = BarLook {
                    min_len: length(bar, "minLength", 0.1),
                    max_len: length(bar, "maxLength", 0.9),
                    ..BarLook::KANTOR
                };
                out.bars.insert((*sheet_id, range.to_owned()), (None, look));
                if let Some(id) = rule
                    .descendants()
                    .find(|n| n.is_element() && n.tag_name().name() == "id")
                    .and_then(|n| n.text())
                {
                    ids.insert(id.trim().to_owned(), range.to_owned());
                }
            }
        }
        let exts = root
            .children()
            .filter(|n| n.is_element() && n.tag_name().name() == "extLst")
            .flat_map(|l| l.children())
            .filter(|e| e.is_element() && e.attribute("uri") == Some(CF_EXTENSION));
        for ext in exts {
            for cf in ext
                .descendants()
                .filter(|n| n.is_element() && n.tag_name().name() == "conditionalFormatting")
            {
                let Some(range) = child(cf, "sqref").and_then(|n| n.text()) else {
                    continue;
                };
                let range = range.trim().to_owned();
                for rule in cf
                    .children()
                    .filter(|n| n.is_element() && n.tag_name().name() == "cfRule")
                {
                    let data_bar = match rule.attribute("type") {
                        Some("dataBar") => true,
                        Some("iconSet") => false,
                        _ => continue,
                    };
                    let bar_node = child(rule, "dataBar");
                    if data_bar && let Some(bar) = bar_node {
                        // The look of the main rule it extends (by id).
                        let main = rule
                            .attribute("id")
                            .and_then(|id| ids.get(id.trim()))
                            .cloned()
                            .unwrap_or_else(|| range.clone());
                        let border = flag(bar, "border", false)
                            .then(|| child(bar, "borderColor").and_then(color_of))
                            .flatten();
                        let same = flag(bar, "negativeBarBorderColorSameAsPositive", true);
                        let look = BarLook {
                            min_len: length(bar, "minLength", 0.1),
                            max_len: length(bar, "maxLength", 0.9),
                            negative_border: if same {
                                None
                            } else {
                                child(bar, "negativeBorderColor").and_then(color_of)
                            },
                            border,
                            negative_fill: if flag(bar, "negativeBarColorSameAsPositive", false) {
                                None
                            } else {
                                child(bar, "negativeFillColor").and_then(color_of)
                            },
                            axis: match bar.attribute("axisPosition") {
                                Some("middle") => Axis::Middle,
                                Some("none") => Axis::None,
                                _ => Axis::Automatic,
                            },
                            axis_color: child(bar, "axisColor").and_then(color_of),
                            direction: match bar.attribute("direction") {
                                Some("leftToRight") => Direction::LeftToRight,
                                Some("rightToLeft") => Direction::RightToLeft,
                                _ => Direction::Context,
                            },
                        };
                        out.bars.insert((*sheet_id, main), (None, look));
                    }
                    let Some(xml) = fragment(cf, src) else {
                        continue;
                    };
                    out.rules.push(X14Rule {
                        sheet_id: *sheet_id,
                        range: range.clone(),
                        data_bar,
                        bar_xml: bar_node.and_then(|b| fragment(b, src)),
                        xml,
                        rule: None,
                    });
                }
            }
        }
    }
    out
}

/// An imported rule's Excel 2010 extras (`.xlsb`), for the model rule of
/// that kind over its range.
#[derive(Debug, Clone)]
pub(crate) enum ImportedLook {
    Bar {
        look: BarLook,
        gradient: bool,
    },
    /// Icons picked one by one: the set named, whether values show, and
    /// each threshold's icon (set, index).
    CustomIcons {
        set: String,
        show_value: bool,
        icons: Vec<(String, u8)>,
    },
}

/// Whether `rule` is the kind an x14 rule of this kind holds.
fn same_kind(rule: &CfRule, data_bar: bool) -> bool {
    if data_bar {
        matches!(rule, CfRule::DataBar { .. })
    } else {
        matches!(rule, CfRule::IconSet { .. } | CfRule::IconRating { .. })
    }
}

impl CfX14 {
    /// Remember the model rule each x14 rule and data bar belongs to, as
    /// opened: `sheets` gives each sheet's `sheetId` and rules.
    pub(crate) fn bind<'a>(
        &mut self,
        sheets: impl IntoIterator<Item = (u32, &'a [ConditionalFormatting])>,
    ) {
        for (id, list) in sheets {
            for r in self.rules.iter_mut().filter(|r| r.sheet_id == id) {
                r.rule = list
                    .iter()
                    .find(|cf| cf.range == r.range && same_kind(&cf.cf_rule, r.data_bar))
                    .map(|cf| cf.cf_rule.clone());
            }
            for ((sheet, range), (rule, _)) in self.bars.iter_mut() {
                if *sheet == id {
                    *rule = list
                        .iter()
                        .find(|cf| cf.range == *range && same_kind(&cf.cf_rule, true))
                        .map(|cf| cf.cf_rule.clone());
                }
            }
        }
        self.rules.retain(|r| r.rule.is_some());
        self.bars.retain(|_, (rule, _)| rule.is_some());
    }

    /// An import's extras (no package to carry them from): drawn like a
    /// file's, and written back by a save (the sheet's rules are written
    /// from the model, so `touched.cf` must hold it).
    pub(crate) fn add_imported(
        &mut self,
        sheet_id: u32,
        range: &str,
        look: ImportedLook,
        list: &[ConditionalFormatting],
    ) {
        let data_bar = matches!(look, ImportedLook::Bar { .. });
        let Some(rule) = list
            .iter()
            .find(|cf| cf.range == range && same_kind(&cf.cf_rule, data_bar))
            .map(|cf| cf.cf_rule.clone())
        else {
            return;
        };
        match look {
            ImportedLook::Bar { look, gradient } => {
                let CfRule::DataBar {
                    min,
                    max,
                    negative_color,
                    ..
                } = &rule
                else {
                    return;
                };
                let bar_xml = look.x14_xml(gradient, min.as_ref(), max.as_ref(), negative_color);
                self.bars
                    .insert((sheet_id, range.to_owned()), (Some(rule.clone()), look));
                self.rules.push(X14Rule {
                    sheet_id,
                    range: range.to_owned(),
                    data_bar: true,
                    xml: String::new(),
                    bar_xml: Some(bar_xml),
                    rule: Some(rule),
                });
            }
            ImportedLook::CustomIcons {
                set,
                show_value,
                icons,
            } => {
                let CfRule::IconSet { thresholds, .. } = &rule else {
                    return;
                };
                if thresholds.len() != icons.len() {
                    return;
                }
                let steps: Vec<_> = thresholds
                    .iter()
                    .zip(icons)
                    .map(|(t, (s, i))| (t.cfvo.clone(), t.is_strict, s, i))
                    .collect();
                self.rules.push(X14Rule {
                    sheet_id,
                    range: range.to_owned(),
                    data_bar: false,
                    xml: custom_icons_xml(range, &set, show_value, &steps),
                    bar_xml: None,
                    rule: Some(rule),
                });
            }
        }
    }

    /// How the data bar `rule` over `range` of sheet `sheet_id` is drawn:
    /// the file's look while the rule is as opened, else Kantor's.
    pub(crate) fn bar_look(&self, sheet_id: u32, range: &str, rule: &CfRule) -> BarLook {
        match self.bars.get(&(sheet_id, range.to_owned())) {
            Some((Some(r), look)) if r == rule => look.clone(),
            _ => BarLook::KANTOR,
        }
    }

    /// The x14 rules of `sheet_id` to put back into a save that writes the
    /// sheet's rules from the model: those whose rule is still in `list`
    /// unchanged, with the range it has now.
    pub(crate) fn kept(&self, sheet_id: u32, list: &[ConditionalFormatting]) -> Vec<KeptX14> {
        self.rules
            .iter()
            .filter(|r| r.sheet_id == sheet_id)
            .filter_map(|r| {
                let now = list
                    .iter()
                    .find(|cf| Some(&cf.cf_rule) == r.rule.as_ref())?;
                Some(KeptX14 {
                    range: now.range.clone(),
                    old_range: r.range.clone(),
                    data_bar: r.data_bar,
                    xml: r.xml.clone(),
                    bar_xml: r.bar_xml.clone(),
                })
            })
            .collect()
    }
}

/// An x14 rule a save puts back.
#[derive(Debug, Clone)]
pub(crate) struct KeptX14 {
    pub range: String,
    pub old_range: String,
    pub data_bar: bool,
    pub xml: String,
    pub bar_xml: Option<String>,
}

/// A worksheet's XML as IronCalc wrote it, with the x14 markup of `kept`
/// rules put back: a data bar's `<x14:dataBar>` in place of IronCalc's (its
/// lengths, borders, axis), an icon set IronCalc couldn't write (custom
/// icons) added to the x14 conditional formats.
pub(crate) fn patch_sheet(xml: &str, kept: &[KeptX14]) -> String {
    let mut out = xml.to_owned();
    for k in kept {
        let sqref = format!("<xm:sqref>{}</xm:sqref>", k.range);
        if k.data_bar {
            let Some(bar) = &k.bar_xml else { continue };
            // IronCalc's x14 block for this range.
            let mut from = 0;
            while let Some(at) = out[from..].find("<x14:conditionalFormatting") {
                let start = from + at;
                let Some(len) = out[start..].find("</x14:conditionalFormatting>") else {
                    break;
                };
                let end = start + len;
                let block = &out[start..end];
                if block.contains(&sqref) && block.contains("type=\"dataBar\"") {
                    if let (Some(a), Some(b)) =
                        (block.find("<x14:dataBar"), block.find("</x14:dataBar>"))
                    {
                        let (a, b) = (start + a, start + b + "</x14:dataBar>".len());
                        out.replace_range(a..b, bar);
                    }
                    break;
                }
                from = end;
            }
        } else {
            // A named icon set IronCalc writes itself; a custom one it writes
            // as a named set: that rule goes, the file's takes its place
            // (and its priority).
            if !k.xml.contains("custom=\"1\"") {
                continue;
            }
            let main = format!("<conditionalFormatting sqref=\"{}\">", k.range);
            let mut priority = None;
            let mut from = 0;
            while let Some(at) = out[from..].find(&main) {
                let start = from + at;
                let Some(len) = out[start..].find("</conditionalFormatting>") else {
                    break;
                };
                let end = start + len + "</conditionalFormatting>".len();
                let block = &out[start..end];
                if block.matches("<cfRule").count() == 1 && block.contains("type=\"iconSet\"") {
                    priority = block
                        .split("priority=\"")
                        .nth(1)
                        .and_then(|r| r.split('"').next())
                        .map(str::to_owned);
                    out.replace_range(start..end, "");
                    break;
                }
                from = end;
            }
            let mut xml = k.xml.clone();
            if let Some(p) = priority
                && let Some(at) = xml.find("priority=\"")
                && let Some(len) = xml[at + 10..].find('"')
            {
                xml.replace_range(at + 10..at + 10 + len, &p);
            }
            let block = xml.replacen(&format!("<xm:sqref>{}</xm:sqref>", k.old_range), &sqref, 1);
            if let Some(at) = out.find("</x14:conditionalFormattings>") {
                out.insert_str(at, &block);
            } else {
                let ext = format!(
                    "<ext uri=\"{CF_EXTENSION}\" xmlns:x14=\"{NS_X14}\"><x14:conditionalFormattings>{block}</x14:conditionalFormattings></ext>"
                );
                if let Some(at) = out.find("<extLst>") {
                    out.insert_str(at + "<extLst>".len(), &ext);
                } else if let Some(at) = out.rfind("</worksheet>") {
                    out.insert_str(at, &format!("<extLst>{ext}</extLst>"));
                }
            }
        }
    }
    out
}
