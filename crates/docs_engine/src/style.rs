//! Styles (`styles.xml`) and resolving a run's effective formatting.
//!
//! Resolution follows OOXML's order: document defaults, the table style
//! (inside a table), the paragraph style's `basedOn` chain from the root
//! down, the list level's indents, the character style, then direct
//! formatting. Toggle-property XOR and conditional table formatting
//! (first row, banding) aren't modelled.

use crate::model::{
    Align, Fonts, Indent, LineRule, LineSpacing, NumRef, ParaProps, Paragraph, Rgb, RunProps,
};
use crate::table::{Borders, CellMargins};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StyleKind {
    #[default]
    Paragraph,
    Character,
    Table,
    Numbering,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Style {
    pub id: String,
    pub name: String,
    pub kind: StyleKind,
    pub based_on: Option<String>,
    /// Style of the paragraph Enter creates after this one.
    pub next: Option<String>,
    /// `w:default="1"`: the style paragraphs without `w:pStyle` use.
    pub is_default: bool,
    pub run: RunProps,
    pub para: ParaProps,
    /// Table styles: borders and cell margins (`w:tblPr`).
    pub table: Option<TableStyle>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct TableStyle {
    pub borders: Borders,
    pub cell_margins: Option<CellMargins>,
}

/// Effective character formatting, ready for layout.
#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedRun {
    /// The document's font name (for display).
    pub font: String,
    pub size_pt: f32,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub color: Rgb,
}

/// Effective line spacing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ResolvedLine {
    /// Multiple of the font's natural line height (1.0 = single).
    Multiple(f32),
    /// At least this many points.
    AtLeast(f32),
    /// Exactly this many points.
    Exact(f32),
}

/// Effective paragraph formatting, ready for layout.
#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedPara {
    pub align: Align,
    pub space_before_pt: f32,
    pub space_after_pt: f32,
    pub line: ResolvedLine,
    pub indent_left_pt: f32,
    pub indent_right_pt: f32,
    /// First-line indent; negative = hanging.
    pub first_line_pt: f32,
    /// The list the paragraph is in (from direct formatting or its style).
    pub num: Option<NumRef>,
    pub page_break_before: bool,
    pub contextual: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct StyleSheet {
    pub default_run: RunProps,
    pub default_para: ParaProps,
    pub styles: Vec<Style>,
    /// Theme fonts (`theme1.xml`): headings (major) and body (minor).
    pub major_font: String,
    pub minor_font: String,
}

impl Default for StyleSheet {
    fn default() -> Self {
        Self {
            default_run: RunProps::default(),
            default_para: ParaProps::default(),
            styles: Vec::new(),
            major_font: "Calibri Light".into(),
            minor_font: "Calibri".into(),
        }
    }
}

/// Heading levels offered in the UI, as built-in style ids.
pub const HEADING_STYLES: [&str; 3] = ["Heading1", "Heading2", "Heading3"];

impl StyleSheet {
    /// Word 2016+'s defaults for a new blank document: Calibri 11 pt, 8 pt
    /// after, 1.08 lines; Title and three heading levels.
    pub fn builtin() -> Self {
        let blue = Rgb([0x2F, 0x54, 0x96]);
        let heading = |id: &str, name: &str, size: u32, before: u32, color: Rgb| Style {
            id: id.into(),
            name: name.into(),
            kind: StyleKind::Paragraph,
            based_on: Some("Normal".into()),
            next: Some("Normal".into()),
            is_default: false,
            run: RunProps {
                fonts: Fonts::family("Calibri Light"),
                size: Some(size),
                color: Some(color),
                ..RunProps::default()
            },
            table: None,
            para: ParaProps {
                before: Some(before),
                after: Some(0),
                line: Some(LineSpacing {
                    line: 259,
                    rule: LineRule::Auto,
                }),
                ..ParaProps::default()
            },
        };
        let mut title = heading("Title", "Title", 56, 0, Rgb([0, 0, 0]));
        title.para.line = Some(LineSpacing {
            line: 240,
            rule: LineRule::Auto,
        });
        Self {
            default_run: RunProps {
                fonts: Fonts::family("Calibri"),
                size: Some(22),
                ..RunProps::default()
            },
            default_para: ParaProps {
                after: Some(160),
                line: Some(LineSpacing {
                    line: 259,
                    rule: LineRule::Auto,
                }),
                ..ParaProps::default()
            },
            styles: vec![
                Style {
                    id: "Normal".into(),
                    name: "Normal".into(),
                    is_default: true,
                    ..Style::default()
                },
                Style {
                    id: "Hyperlink".into(),
                    name: "Hyperlink".into(),
                    kind: StyleKind::Character,
                    run: RunProps {
                        color: Some(Rgb([0x05, 0x63, 0xC1])),
                        underline: Some(true),
                        ..RunProps::default()
                    },
                    ..Style::default()
                },
                title,
                heading("Heading1", "heading 1", 32, 240, blue),
                heading("Heading2", "heading 2", 26, 40, blue),
                heading("Heading3", "heading 3", 24, 40, Rgb([0x1F, 0x37, 0x63])),
            ],
            major_font: "Calibri Light".into(),
            minor_font: "Calibri".into(),
        }
    }

    pub fn get(&self, id: &str) -> Option<&Style> {
        self.styles.iter().find(|s| s.id == id)
    }

    fn default_para_style(&self) -> Option<&Style> {
        self.styles
            .iter()
            .find(|s| s.is_default && s.kind == StyleKind::Paragraph)
    }

    /// The style chain for a paragraph, root first.
    fn chain(&self, style: Option<&str>) -> Vec<&Style> {
        let start = match style {
            Some(id) => self.get(id).or_else(|| self.default_para_style()),
            None => self.default_para_style(),
        };
        self.chain_from(start)
    }

    /// The `basedOn` chain of a style, root first (empty if unknown).
    fn chain_from<'a>(&'a self, start: Option<&'a Style>) -> Vec<&'a Style> {
        let mut out = Vec::new();
        let mut cur = start;
        while let Some(s) = cur {
            if out.len() > 16 || out.iter().any(|o: &&Style| o.id == s.id) {
                break; // a cycle in a broken file
            }
            out.push(s);
            cur = s.based_on.as_deref().and_then(|b| self.get(b));
        }
        out.reverse();
        out
    }

    /// Display name of a style id (`Heading1` → `heading 1`).
    pub fn display_name(&self, id: &str) -> String {
        self.get(id)
            .map(|s| s.name.clone())
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| id.to_owned())
    }

    /// The style Enter gives the next paragraph after one in `style`.
    pub fn next_style(&self, style: Option<&str>) -> Option<String> {
        let s = self.get(style?)?;
        match s.next.as_deref() {
            Some(n) if n != s.id => Some(n.to_owned()),
            Some(_) => Some(s.id.clone()),
            None => Some(s.id.clone()),
        }
    }

    pub fn resolve_para(&self, para: &Paragraph) -> ResolvedPara {
        self.resolve_para_with(para, None, &|_| None)
    }

    /// The list a paragraph is in: direct `w:numPr`, else its style's.
    pub fn effective_num(&self, para: &Paragraph) -> Option<NumRef> {
        let mut num = None;
        for s in self.chain(para.props.style.as_deref()) {
            if s.para.num.is_some() {
                num = s.para.num;
            }
        }
        if para.props.num.is_some() {
            num = para.props.num;
        }
        num.filter(|n| n.id != 0)
    }

    /// Effective paragraph formatting inside a table with `table_style`;
    /// `list_indent` gives a list level's indents (`numbering.xml`).
    pub fn resolve_para_with(
        &self,
        para: &Paragraph,
        table_style: Option<&str>,
        list_indent: &dyn Fn(NumRef) -> Option<Indent>,
    ) -> ResolvedPara {
        let mut p = self.default_para.clone();
        for s in self.chain_from(table_style.and_then(|t| self.get(t))) {
            p.overlay(&s.para);
        }
        for s in self.chain(para.props.style.as_deref()) {
            p.overlay(&s.para);
        }
        let num = self.effective_num(para);
        if let Some(ind) = num.and_then(list_indent) {
            p.indent.overlay(&ind);
        }
        p.overlay(&para.props);
        let twips_pt = |t: u32| t as f32 / 20.0;
        let itw = |t: Option<i32>| t.unwrap_or(0) as f32 / 20.0;
        let line = match p.line {
            None => ResolvedLine::Multiple(1.0),
            Some(LineSpacing {
                line,
                rule: LineRule::Auto,
            }) => ResolvedLine::Multiple((line as f32 / 240.0).max(0.1)),
            Some(LineSpacing {
                line,
                rule: LineRule::AtLeast,
            }) => ResolvedLine::AtLeast(line.max(0) as f32 / 20.0),
            Some(LineSpacing {
                line,
                rule: LineRule::Exact,
            }) => ResolvedLine::Exact(line.max(0) as f32 / 20.0),
        };
        ResolvedPara {
            align: p.align.unwrap_or_default(),
            space_before_pt: twips_pt(p.before.unwrap_or(0)),
            space_after_pt: twips_pt(p.after.unwrap_or(0)),
            line,
            indent_left_pt: itw(p.indent.left),
            indent_right_pt: itw(p.indent.right),
            first_line_pt: itw(p.indent.first),
            num,
            page_break_before: p.page_break_before.unwrap_or(false),
            contextual: p.contextual.unwrap_or(false),
        }
    }

    /// Borders and cell margins of a table style, through `basedOn`.
    pub fn table_style(&self, id: Option<&str>) -> TableStyle {
        let mut out = TableStyle::default();
        for s in self.chain_from(id.and_then(|i| self.get(i))) {
            if let Some(t) = &s.table {
                out.borders.overlay(&t.borders);
                if t.cell_margins.is_some() {
                    out.cell_margins = t.cell_margins;
                }
            }
        }
        out
    }

    /// Formatting of `run` in `para` after the style chain.
    pub fn resolve_run(&self, para: &Paragraph, run: &RunProps) -> ResolvedRun {
        self.resolve_run_with(para, run, None)
    }

    /// Formatting of `run` in `para` inside a table with `table_style`.
    pub fn resolve_run_with(
        &self,
        para: &Paragraph,
        run: &RunProps,
        table_style: Option<&str>,
    ) -> ResolvedRun {
        let mut r = self.default_run.clone();
        for s in self.chain_from(table_style.and_then(|t| self.get(t))) {
            r.overlay(&s.run);
        }
        for s in self.chain(para.props.style.as_deref()) {
            r.overlay(&s.run);
        }
        for s in self.chain_from(run.style.as_deref().and_then(|c| self.get(c))) {
            r.overlay(&s.run);
        }
        r.overlay(run);
        let font = match (&r.fonts.ascii, r.fonts.ascii_theme.as_deref()) {
            (_, Some(t)) if t.starts_with("major") => self.major_font.clone(),
            (_, Some(t)) if t.starts_with("minor") => self.minor_font.clone(),
            (Some(f), _) => f.clone(),
            _ => self.minor_font.clone(),
        };
        ResolvedRun {
            font,
            size_pt: r.size.unwrap_or(20) as f32 / 2.0,
            bold: r.bold.unwrap_or(false),
            italic: r.italic.unwrap_or(false),
            underline: r.underline.unwrap_or(false),
            color: r.color.unwrap_or(Rgb([0, 0, 0])),
        }
    }
}

/// A CSS-style font stack for a document font: the font itself first (used
/// when installed), then its metric-compatible free substitute (plan §5),
/// then broad fallbacks for other scripts.
pub fn font_stack(font: &str) -> String {
    let substitute = match font.to_ascii_lowercase().as_str() {
        "calibri" | "calibri light" => Some("Carlito"),
        "cambria" | "cambria math" => Some("Caladea"),
        "arial" | "helvetica" => Some("Liberation Sans"),
        "times new roman" | "times" => Some("Liberation Serif"),
        "courier new" | "courier" => Some("Liberation Mono"),
        _ => None,
    };
    let mut s = String::new();
    s.push('"');
    s.push_str(&font.replace('"', ""));
    s.push('"');
    if let Some(sub) = substitute {
        s.push_str(", ");
        s.push_str(sub);
    }
    s.push_str(
        ", Carlito, Liberation Sans, Noto Sans, Noto Sans CJK SC, Noto Sans Arabic, sans-serif",
    );
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heading_resolves_through_based_on() {
        let s = StyleSheet::builtin();
        let mut p = Paragraph::new("Title").with_style("Heading1");
        let r = s.resolve_run(&p, &p.runs[0].props.clone());
        assert_eq!(r.size_pt, 16.0);
        assert_eq!(r.font, "Calibri Light");
        assert_eq!(r.color, Rgb([0x2F, 0x54, 0x96]));
        let para = s.resolve_para(&p);
        assert_eq!(para.space_before_pt, 12.0);
        assert_eq!(para.space_after_pt, 0.0);

        p.props.style = None;
        let r = s.resolve_run(&p, &RunProps::default());
        assert_eq!(r.size_pt, 11.0);
        assert_eq!(r.font, "Calibri");
        assert_eq!(s.resolve_para(&p).space_after_pt, 8.0);
    }

    #[test]
    fn direct_formatting_wins() {
        let s = StyleSheet::builtin();
        let p = Paragraph::new("x").with_style("Heading1");
        let run = RunProps {
            size: Some(40),
            bold: Some(true),
            fonts: Fonts::family("Arial"),
            ..RunProps::default()
        };
        let r = s.resolve_run(&p, &run);
        assert_eq!((r.size_pt, r.bold, r.font.as_str()), (20.0, true, "Arial"));
    }

    #[test]
    fn theme_fonts_and_stacks() {
        let s = StyleSheet::builtin();
        let run = RunProps {
            fonts: Fonts {
                ascii_theme: Some("majorHAnsi".into()),
                ..Fonts::default()
            },
            ..RunProps::default()
        };
        assert_eq!(
            s.resolve_run(&Paragraph::new(""), &run).font,
            "Calibri Light"
        );
        assert!(font_stack("Calibri").starts_with("\"Calibri\", Carlito"));
        assert_eq!(s.next_style(Some("Heading1")).as_deref(), Some("Normal"));
    }
}
