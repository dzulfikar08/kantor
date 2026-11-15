//! Page setup and defined names of OpenDocument spreadsheets (`.ods`).
//!
//! * Page setup: a table's style names its master page (`styles.xml`),
//!   whose page layout gives the paper, orientation, margins, scaling (a
//!   percentage, or fit to pages wide and tall), page order, first page
//!   number, centring and what is printed (grid, headings); its header and
//!   footer (left, centre and right regions, with sheet name, page number,
//!   page count, file name, date and time fields) become Excel's `&` codes.
//!   The print area is the table's `table:print-ranges`; print titles are
//!   its header rows and columns ([`crate::ods`] finds them).
//! * Defined names: `table:named-range` (cell ranges) and
//!   `table:named-expression` (formulas) of the workbook and of each table
//!   (local names). The `_xlnm.` names LibreOffice writes for print areas
//!   and titles are left to the page setup.

use std::collections::HashMap;

use roxmltree::{Document, Node};

use crate::print::{HeaderFooter, Margins, PageSetup, Paper, PrintOptions, SheetPrint};

const NS_STYLE: &str = "urn:oasis:names:tc:opendocument:xmlns:style:1.0";
const NS_FO: &str = "urn:oasis:names:tc:opendocument:xmlns:xsl-fo-compatible:1.0";
const NS_TEXT: &str = "urn:oasis:names:tc:opendocument:xmlns:text:1.0";
const NS_TABLE: &str = "urn:oasis:names:tc:opendocument:xmlns:table:1.0";

/// A length in inches.
fn inches(s: &str) -> Option<f64> {
    let s = s.trim();
    let split = s.find(|c: char| c.is_ascii_alphabetic())?;
    let (n, unit) = s.split_at(split);
    let n: f64 = n.trim().parse().ok()?;
    let k = match unit {
        "in" => 1.0,
        "cm" => 1.0 / 2.54,
        "mm" => 1.0 / 25.4,
        "pt" => 1.0 / 72.0,
        "pc" => 1.0 / 6.0,
        "px" => 1.0 / 96.0,
        _ => return None,
    };
    let v = n * k;
    v.is_finite().then_some(v)
}

/// Page layouts and master pages of the documents.
pub(crate) struct Pages<'a, 'i> {
    /// Table style → master page name.
    table_master: HashMap<&'a str, &'a str>,
    masters: HashMap<&'a str, Node<'a, 'i>>,
    layouts: HashMap<&'a str, Node<'a, 'i>>,
}

impl<'a, 'i> Pages<'a, 'i> {
    pub(crate) fn read(docs: &[&'a Document<'i>]) -> Self {
        let mut p = Pages {
            table_master: HashMap::new(),
            masters: HashMap::new(),
            layouts: HashMap::new(),
        };
        for doc in docs {
            for n in doc.descendants().filter(|n| n.is_element()) {
                let Some(name) = n.attribute((NS_STYLE, "name")) else {
                    continue;
                };
                if n.has_tag_name((NS_STYLE, "style"))
                    && n.attribute((NS_STYLE, "family")) == Some("table")
                    && let Some(m) = n.attribute((NS_STYLE, "master-page-name"))
                {
                    p.table_master.insert(name, m);
                } else if n.has_tag_name((NS_STYLE, "master-page")) {
                    p.masters.insert(name, n);
                } else if n.has_tag_name((NS_STYLE, "page-layout")) {
                    p.layouts.insert(name, n);
                }
            }
        }
        p
    }

    /// The page setup of a table with style `table_style` (`None`: the
    /// default master page's).
    pub(crate) fn print(&self, table_style: Option<&str>) -> SheetPrint {
        let mut out = SheetPrint {
            header_footer: HeaderFooter {
                scale_with_doc: true,
                align_with_margins: true,
                ..Default::default()
            },
            ..Default::default()
        };
        let master_name = table_style
            .and_then(|s| self.table_master.get(s).copied())
            .unwrap_or("Default");
        let Some(master) = self
            .masters
            .get(master_name)
            .or_else(|| self.masters.get("Default"))
            .copied()
        else {
            return out;
        };
        let layout = master
            .attribute((NS_STYLE, "page-layout-name"))
            .and_then(|l| self.layouts.get(l))
            .copied();
        let props = layout.and_then(|l| {
            l.children()
                .find(|c| c.has_tag_name((NS_STYLE, "page-layout-properties")))
        });
        let mut setup = PageSetup::default();
        let mut margins = Margins::default();
        let mut options = PrintOptions::default();
        if let Some(pp) = props {
            let fo = |a: &str| pp.attribute((NS_FO, a));
            let st = |a: &str| pp.attribute((NS_STYLE, a));
            if let (Some(w), Some(h)) = (
                fo("page-width").and_then(inches),
                fo("page-height").and_then(inches),
            ) && w > 0.0
                && h > 0.0
            {
                let (a, b) = (w.min(h) * 25.4, w.max(h) * 25.4);
                // Snap to a known paper, so `paperSize` gets its code.
                setup.paper = [
                    Paper::A4,
                    Paper::LETTER,
                    Paper::LEGAL,
                    Paper::FOLIO,
                    Paper::A3,
                    Paper::A5,
                ]
                .into_iter()
                .find(|p| (p.width_mm - a).abs() < 1.0 && (p.height_mm - b).abs() < 1.0)
                .unwrap_or(Paper::mm(a, b));
                setup.landscape = w > h;
            }
            match st("print-orientation") {
                Some("landscape") => setup.landscape = true,
                Some("portrait") => setup.landscape = false,
                _ => {}
            }
            if let Some(s) =
                st("scale-to").and_then(|s| s.trim().trim_end_matches('%').parse::<f64>().ok())
                && (10.0..=400.0).contains(&s)
            {
                setup.scale = s.round() as u32;
            }
            let pages = |a: &str| st(a).and_then(|v| v.parse::<u32>().ok());
            if let (x, y) = (pages("scale-to-X"), pages("scale-to-Y"))
                && (x.is_some() || y.is_some())
            {
                setup.fit = Some((x.unwrap_or(0), y.unwrap_or(0)));
            } else if let Some(n) = pages("scale-to-pages") {
                setup.fit = Some((n, n));
            }
            setup.over_then_down = st("print-page-order") == Some("ltr");
            setup.first_page_number = st("first-page-number").and_then(|v| v.parse().ok());
            let print = st("print").unwrap_or("");
            options.grid_lines = print.split_whitespace().any(|w| w == "grid");
            options.headings = print.split_whitespace().any(|w| w == "headers");
            match st("table-centering") {
                Some("horizontal") => options.horizontal_centered = true,
                Some("vertical") => options.vertical_centered = true,
                Some("both") => {
                    options.horizontal_centered = true;
                    options.vertical_centered = true;
                }
                _ => {}
            }
            let m = |a: &str| fo(a).and_then(inches);
            margins.left = m("margin-left").unwrap_or(0.7874);
            margins.right = m("margin-right").unwrap_or(0.7874);
            margins.top = m("margin-top").unwrap_or(0.7874);
            margins.bottom = m("margin-bottom").unwrap_or(0.7874);
        }
        // LibreOffice's page margin is the edge of the header; Excel's top
        // margin is the body's and its header margin the header's.
        let band = |tag: &str| -> f64 {
            layout
                .and_then(|l| l.children().find(|c| c.has_tag_name((NS_STYLE, tag))))
                .and_then(|h| {
                    h.children()
                        .find(|c| c.has_tag_name((NS_STYLE, "header-footer-properties")))
                })
                .map_or(0.0, |p| {
                    let a = |n: &str| p.attribute((NS_FO, n)).and_then(inches).unwrap_or(0.0);
                    a("min-height") + a("margin-bottom") + a("margin-top")
                })
        };
        let header = region_text(master, "header");
        let footer = region_text(master, "footer");
        margins.header = margins.top;
        margins.footer = margins.bottom;
        if header.is_some() {
            margins.top += band("header-style");
        }
        if footer.is_some() {
            margins.bottom += band("footer-style");
        }
        out.setup = setup;
        out.margins = margins;
        out.options = options;
        out.header_footer.odd_header = header.unwrap_or_default();
        out.header_footer.odd_footer = footer.unwrap_or_default();
        out
    }
}

/// A master page's header or footer as Excel's codes; `None` when it isn't
/// shown.
fn region_text(master: Node, tag: &str) -> Option<String> {
    let h = master
        .children()
        .find(|c| c.has_tag_name((NS_STYLE, tag)))?;
    if h.attribute((NS_STYLE, "display")) == Some("false") {
        return None;
    }
    let mut out = String::new();
    let regions = [
        ("region-left", "&L"),
        ("region-center", "&C"),
        ("region-right", "&R"),
    ];
    if regions
        .iter()
        .any(|(r, _)| h.children().any(|c| c.has_tag_name((NS_STYLE, *r))))
    {
        for (r, code) in regions {
            if let Some(region) = h.children().find(|c| c.has_tag_name((NS_STYLE, r))) {
                let t = paragraphs(region);
                if !t.is_empty() {
                    out.push_str(code);
                    out.push_str(&t);
                }
            }
        }
    } else {
        let t = paragraphs(h);
        if !t.is_empty() {
            out.push_str("&C");
            out.push_str(&t);
        }
    }
    Some(out)
}

fn paragraphs(n: Node) -> String {
    let lines: Vec<String> = n
        .children()
        .filter(|c| c.has_tag_name((NS_TEXT, "p")))
        .map(|p| {
            let mut s = String::new();
            codes(p, &mut s);
            s
        })
        .collect();
    lines.join("\n")
}

fn codes(n: Node, out: &mut String) {
    for c in n.children() {
        if c.is_text() {
            out.push_str(&c.text().unwrap_or_default().replace('&', "&&"));
            continue;
        }
        if c.tag_name().namespace() != Some(NS_TEXT) {
            continue;
        }
        match c.tag_name().name() {
            "sheet-name" => out.push_str("&A"),
            "page-number" => out.push_str("&P"),
            "page-count" => out.push_str("&N"),
            "file-name" | "title" => out.push_str("&F"),
            "date" => out.push_str("&D"),
            "time" => out.push_str("&T"),
            "s" => {
                let k: usize = c
                    .attribute((NS_TEXT, "c"))
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(1);
                out.push_str(&" ".repeat(k.min(50)));
            }
            "tab" => out.push(' '),
            "line-break" => out.push('\n'),
            _ => codes(c, out),
        }
    }
}

/// A defined name of the file: name, local sheet (index into the file's
/// tables), formula (Excel syntax, without `=`).
pub(crate) type Name = (String, Option<u32>, String);

/// The defined names of `content` (the text of `content.xml`), sheet names
/// in references through `rename`. Only the `table:named-expressions`
/// elements are parsed, not the whole document.
pub(crate) fn defined_names(
    content: &str,
    tables: &[String],
    rename: &dyn Fn(&str) -> String,
) -> Vec<Name> {
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(i) = content[from..].find("<table:named-expressions>") {
        let start = from + i;
        let Some(len) = content[start..].find("</table:named-expressions>") else {
            break;
        };
        let end = start + len + "</table:named-expressions>".len();
        from = end;
        // A table's own names: inside its element.
        let before = &content[..start];
        let open = before.rfind("<table:table ");
        let close = before.rfind("</table:table>");
        let sheet = match (open, close) {
            (Some(o), c) if c.is_none_or(|c| c < o) => {
                let tag_end = before[o..].find('>').map_or(before.len(), |e| o + e);
                let tag = &before[o..tag_end];
                tag.split("table:name=\"")
                    .nth(1)
                    .and_then(|r| r.split('"').next())
                    .map(unescape)
                    .and_then(|n| tables.iter().position(|t| *t == n))
                    .map(|i| i as u32)
            }
            _ => None,
        };
        let xml = format!("<r xmlns:table=\"{NS_TABLE}\">{}</r>", &content[start..end]);
        let Ok(doc) = Document::parse(&xml) else {
            continue;
        };
        for n in doc.descendants().filter(|n| n.is_element()) {
            let Some(name) = n.attribute((NS_TABLE, "name")) else {
                continue;
            };
            if name.to_ascii_lowercase().starts_with("_xlnm.") {
                continue;
            }
            let formula = if n.has_tag_name((NS_TABLE, "named-range")) {
                let Some(addr) = n.attribute((NS_TABLE, "cell-range-address")) else {
                    continue;
                };
                let refs: Option<Vec<String>> = addr
                    .split_whitespace()
                    .map(|a| crate::ods_drawing::excel_ref(a, rename))
                    .collect();
                match refs {
                    Some(r) if !r.is_empty() => r.join(","),
                    _ => continue,
                }
            } else if n.has_tag_name((NS_TABLE, "named-expression")) {
                let Some(e) = n.attribute((NS_TABLE, "expression")) else {
                    continue;
                };
                let body = e
                    .split_once(":=")
                    .map_or(e.trim_start_matches('='), |(_, b)| b);
                crate::import::open_formula_to_excel(body)
            } else {
                continue;
            };
            out.push((name.to_owned(), sheet, formula));
        }
    }
    out
}

fn unescape(s: &str) -> String {
    s.replace("&apos;", "'")
        .replace("&quot;", "\"")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_by_scope() {
        let content = r#"<x><table:table table:name="Report"><table:named-expressions><table:named-range table:name="Local" table:cell-range-address="$Report.$B$3:.$B$5"/></table:named-expressions></table:table><table:named-expressions><table:named-range table:name="Rate" table:cell-range-address="$Report.$F$6"/><table:named-range table:name="_xlnm.Print_Area" table:cell-range-address="$Report.$A$1:.$F$16"/><table:named-expression table:name="Twice" table:expression="of:=[$Report.$F$6]*2"/></table:named-expressions></x>"#;
        let names = defined_names(content, &["Report".into()], &|n| n.to_owned());
        assert_eq!(
            names,
            [
                ("Local".into(), Some(0), "Report!$B$3:$B$5".into()),
                ("Rate".into(), None, "Report!$F$6".into()),
                ("Twice".into(), None, "Report!$F$6*2".into()),
            ]
        );
    }
}
