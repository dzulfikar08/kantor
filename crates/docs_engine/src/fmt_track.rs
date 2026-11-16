//! Tracked formatting changes (Word's `w:rPrChange` / `w:pPrChange`).
//!
//! With Track Changes on, changing the character or paragraph formatting
//! of text records the old properties next to the new ones, by the user
//! and dated, as Word does: the run's `w:rPr` gets a `w:rPrChange` holding
//! the old `w:rPr`, the paragraph's `w:pPr` a `w:pPrChange` holding the old
//! `w:pPr`. Both ride in the property extras (written back as they are).
//! Accepting one drops the record; rejecting it puts the old properties
//! back. Formatting text the user inserted with Track Changes on isn't
//! recorded (it's new text), and changing formatting back to what it was
//! drops the record, as in Word.

use crate::document::Document;
use crate::model::{Align, ParaProps, Paragraph, RunProps, element_name};
use crate::review::{RevKind, now_utc};
use crate::xml;

/// One thing a formatting change did, for its balloon ("Formatted: Bold").
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FormatItem {
    Bold(bool),
    Italic(bool),
    Underline(bool),
    /// A font face (`None`: back to the style's).
    Font(Option<String>),
    /// Font size in half-points (`None`: back to the style's).
    Size(Option<u32>),
    FontColor,
    /// A character style by name.
    CharStyle(String),
    /// A paragraph style by name.
    Style(String),
    Align(Align),
    Indent,
    Spacing,
    LineSpacing,
    /// A list (bulleted or numbered) on or off.
    List(bool),
    PageBreakBefore,
    TabStops,
    /// A table's properties (`w:tblPrChange`) or its columns' widths
    /// (`w:tblGridChange`).
    TableProps,
    /// Row properties (`w:trPrChange`).
    RowProps,
    /// Cell properties (`w:tcPrChange`).
    CellProps,
    /// Cells merged vertically (`w:cellMerge`): Word's "Merged Cells".
    MergedCells,
    /// Something else (borders, shading, effects Kantor keeps verbatim).
    Other,
}

/// Run properties without revision records (what's compared and stored).
fn plain_run(p: &RunProps) -> RunProps {
    let mut p = p.clone();
    for n in ["rPrChange", "ins", "del", "moveFrom", "moveTo"] {
        p.extras.remove(n);
    }
    p
}

/// Paragraph properties without the mark's run properties, section
/// properties and revision record.
fn plain_para(p: &ParaProps) -> ParaProps {
    let mut p = p.clone();
    for n in ["rPr", "sectPr", "pPrChange"] {
        p.extras.remove(n);
    }
    p
}

/// The old run properties a `w:rPrChange` holds.
pub(crate) fn old_run_props(change: &str) -> RunProps {
    let mut props = RunProps::default();
    if let Some(inner) = inner(change, "rPr") {
        crate::docx::parse_rpr_into(&inner, &mut props);
    }
    props
}

/// The old paragraph properties a `w:pPrChange` holds.
pub(crate) fn old_para_props(change: &str) -> ParaProps {
    let mut props = ParaProps::default();
    if let Some(inner) = inner(change, "pPr") {
        crate::docx::parse_ppr_into(&inner, &mut props);
    }
    props
}

/// The inner property element of a change record (`<w:rPr>…</w:rPr>` or
/// `<w:rPr/>`).
pub(crate) fn inner(change: &str, name: &str) -> Option<String> {
    let open = change
        .find(&format!("<w:{name}>"))
        .or_else(|| change.find(&format!("<w:{name} ")))
        .or_else(|| change.find(&format!("<w:{name}/")))?;
    let close = format!("</w:{name}>");
    match change[open..].find(&close) {
        Some(e) => Some(change[open..open + e + close.len()].to_owned()),
        None => {
            let end = change[open..].find("/>")? + open + 2;
            Some(change[open..end].to_owned())
        }
    }
}

/// The change record on run properties, if any.
pub(crate) fn run_change(p: &RunProps) -> Option<&str> {
    p.extras.iter().find(|x| element_name(x) == "rPrChange")
}

/// The change record on paragraph properties, if any.
pub(crate) fn para_change(p: &ParaProps) -> Option<&str> {
    p.extras.iter().find(|x| element_name(x) == "pPrChange")
}

/// Author and date of a change record.
pub(crate) fn who_when(change: &str) -> (String, String) {
    let tag_end = change.find('>').map_or(change.len(), |i| i + 1);
    let tag = &change[..tag_end];
    let el = xml::parse(&format!(
        "{}/>",
        tag.trim_end().trim_end_matches("/>").trim_end_matches('>')
    ));
    match el {
        Some(el) => (
            el.attr("author").unwrap_or_default().to_owned(),
            el.attr("date").unwrap_or_default().to_owned(),
        ),
        None => (String::new(), String::new()),
    }
}

/// Accept or reject a run's formatting change (keeping a paragraph mark's
/// own `w:ins` / `w:del`). Returns whether there was one.
pub(crate) fn resolve_run(props: &mut RunProps, accept: bool) -> bool {
    let Some(change) = run_change(props).map(str::to_owned) else {
        return false;
    };
    if accept {
        props.extras.remove("rPrChange");
    } else {
        let mut old = old_run_props(&change);
        for x in props.extras.iter() {
            if matches!(element_name(x), "ins" | "del" | "moveFrom" | "moveTo") {
                old.extras.push(x);
            }
        }
        *props = old;
    }
    true
}

/// Accept or reject a paragraph's formatting change (keeping its mark's
/// run properties and section properties). Returns whether there was one.
pub(crate) fn resolve_para(props: &mut ParaProps, accept: bool) -> bool {
    let Some(change) = para_change(props).map(str::to_owned) else {
        return false;
    };
    if accept {
        props.extras.remove("pPrChange");
    } else {
        let mut old = old_para_props(&change);
        old.extras.remove("rPr");
        old.extras.remove("sectPr");
        for x in props.extras.iter() {
            if matches!(element_name(x), "rPr" | "sectPr") {
                old.extras.push(x);
            }
        }
        *props = old;
    }
    true
}

impl Document {
    /// With Track Changes on, record in `new` (the edited copies of
    /// paragraphs `first..`) the formatting they had before. Paragraphs
    /// whose text changed are left alone.
    pub(crate) fn track_formats(&self, first: usize, new: &mut [Paragraph]) {
        if !self.track {
            return;
        }
        let mut id = self.max_rev_id();
        let date = now_utc();
        let author = self.author.clone();
        let mut record = |name: &str, old: &str| {
            id += 1;
            format!(
                "<w:{name} w:id=\"{id}\" w:author=\"{}\" w:date=\"{date}\">{old}</w:{name}>",
                xml::escape(&author)
            )
        };
        for (k, p) in new.iter_mut().enumerate() {
            let Some(old) = self.paragraphs.get(first + k) else {
                break;
            };
            if old.text != p.text {
                continue;
            }
            // Paragraph formatting.
            let (was, now) = (plain_para(&old.props), plain_para(&p.props));
            if was != now {
                match para_change(&p.props).map(str::to_owned) {
                    Some(c) => {
                        // Back to what it was: no change any more.
                        let orig = plain_para(&old_para_props(&c));
                        if same_para(&orig, &now) {
                            p.props.extras.remove("pPrChange");
                        }
                    }
                    None => {
                        let el = record("pPrChange", &para_xml(&was));
                        p.props.extras.push(&el);
                    }
                }
            }
            if p.text.is_empty() {
                continue;
            }
            // Character formatting, run by run (cut where the old runs
            // were, so each piece has one old formatting).
            for (r, _) in old.run_ranges() {
                p.split_run_at(r.start);
            }
            let mut at = 0;
            for run in &mut p.runs {
                let r = at..at + run.len;
                at = r.end;
                if r.is_empty() || run.object.is_some() {
                    continue;
                }
                // Text the user inserted with tracking on is new text.
                if run
                    .rev
                    .as_ref()
                    .is_some_and(|v| v.kind == RevKind::Insert && v.author == author)
                {
                    continue;
                }
                let Some(old_run) = old
                    .run_ranges()
                    .find(|(o, _)| o.start <= r.start && r.start < o.end)
                    .map(|(_, run)| run)
                else {
                    continue;
                };
                let (was, now) = (plain_run(&old_run.props), plain_run(&run.props));
                if self.same_run(old, &was, &now) {
                    continue;
                }
                match run_change(&run.props).map(str::to_owned) {
                    Some(c) => {
                        if self.same_run(old, &plain_run(&old_run_props(&c)), &now) {
                            run.props.extras.remove("rPrChange");
                        }
                    }
                    None => {
                        let mut xml = crate::docx::rpr_xml(&was);
                        if xml.is_empty() {
                            xml = "<w:rPr/>".to_owned();
                        }
                        let el = record("rPrChange", &xml);
                        run.props.extras.push(&el);
                    }
                }
            }
            p.normalize();
        }
    }

    /// Whether two run formattings look the same (`b` off and no `b` in a
    /// plain style are the same) and keep the same extras.
    fn same_run(&self, p: &Paragraph, a: &RunProps, b: &RunProps) -> bool {
        a == b
            || (a.extras == b.extras
                && a.style == b.style
                && self.resolve_run(p, a) == self.resolve_run(p, b))
    }

    /// What a run's formatting change did.
    pub(crate) fn run_format_items(&self, now: &RunProps, change: &str) -> Vec<FormatItem> {
        let was = plain_run(&old_run_props(change));
        let now = plain_run(now);
        let mut out = Vec::new();
        let on = |v: Option<bool>| v.unwrap_or(false);
        if on(was.bold) != on(now.bold) {
            out.push(FormatItem::Bold(on(now.bold)));
        }
        if on(was.italic) != on(now.italic) {
            out.push(FormatItem::Italic(on(now.italic)));
        }
        if on(was.underline) != on(now.underline) {
            out.push(FormatItem::Underline(on(now.underline)));
        }
        if was.fonts != now.fonts {
            let face = now
                .fonts
                .ascii
                .clone()
                .or_else(|| now.fonts.h_ansi.clone())
                .or_else(|| now.fonts.ascii_theme.clone());
            out.push(FormatItem::Font(face));
        }
        if was.size != now.size {
            out.push(FormatItem::Size(now.size));
        }
        if was.color != now.color {
            out.push(FormatItem::FontColor);
        }
        if was.style != now.style {
            let name = now
                .style
                .as_deref()
                .map(|s| self.styles.get(s).map_or(s, |st| st.name.as_str()))
                .unwrap_or("Default Paragraph Font");
            out.push(FormatItem::CharStyle(name.to_owned()));
        }
        if was.extras != now.extras && out.is_empty() {
            out.push(FormatItem::Other);
        }
        out
    }

    /// What a paragraph's formatting change did.
    pub(crate) fn para_format_items(&self, now: &ParaProps, change: &str) -> Vec<FormatItem> {
        let was = plain_para(&old_para_props(change));
        let now = plain_para(now);
        let mut out = Vec::new();
        if was.style != now.style {
            let name = now
                .style
                .as_deref()
                .map(|s| self.styles.get(s).map_or(s, |st| st.name.as_str()))
                .unwrap_or("Normal");
            out.push(FormatItem::Style(name.to_owned()));
        }
        if was.align != now.align {
            out.push(FormatItem::Align(now.align.unwrap_or_default()));
        }
        if was.indent != now.indent || has_changed(&was, &now, "ind") {
            out.push(FormatItem::Indent);
        }
        if was.before != now.before || was.after != now.after {
            out.push(FormatItem::Spacing);
        }
        if was.line != now.line {
            out.push(FormatItem::LineSpacing);
        }
        if was.num != now.num {
            out.push(FormatItem::List(now.num.is_some_and(|n| n.id != 0)));
        }
        if was.page_break_before != now.page_break_before {
            out.push(FormatItem::PageBreakBefore);
        }
        if has_changed(&was, &now, "tabs") {
            out.push(FormatItem::TabStops);
        }
        if out.is_empty() {
            out.push(FormatItem::Other);
        }
        out
    }
}

fn has_changed(was: &ParaProps, now: &ParaProps, name: &str) -> bool {
    let get = |p: &ParaProps| {
        p.extras
            .iter()
            .filter(|x| element_name(x) == name)
            .map(str::to_owned)
            .collect::<Vec<_>>()
    };
    get(was) != get(now)
}

/// Paragraph properties equal as written (a modelled value and the same
/// value kept verbatim compare equal).
fn same_para(a: &ParaProps, b: &ParaProps) -> bool {
    para_xml(a) == para_xml(b)
}

/// `<w:pPr>…</w:pPr>` for paragraph properties (`<w:pPr/>` when empty).
fn para_xml(p: &ParaProps) -> String {
    let para = Paragraph {
        props: p.clone(),
        ..Paragraph::new("x")
    };
    let x = crate::docx::ppr_xml(&para);
    if x.is_empty() { "<w:pPr/>".into() } else { x }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Position, Selection};
    use crate::{Change, Toggle};

    fn doc(paras: &[&str]) -> Document {
        let mut d = Document::from_paragraphs(paras.iter().map(|t| Paragraph::new(*t)).collect());
        d.set_author("Ani Wijaya");
        d.set_track_changes(true, Selection::default());
        d
    }

    fn sel(p: usize, a: usize, b: usize) -> Selection {
        Selection {
            anchor: Position::new(p, a),
            head: Position::new(p, b),
        }
    }

    fn formats(d: &Document) -> Vec<Change> {
        d.changes()
            .into_iter()
            .filter(|c| c.kind == RevKind::Format)
            .collect()
    }

    #[test]
    fn bold_is_recorded_and_rejected() {
        let mut d = doc(&["hello world"]);
        d.toggle(sel(0, 0, 5), Toggle::Bold);
        let f = formats(&d);
        assert_eq!(f.len(), 1, "{f:?}");
        assert_eq!(f[0].format, [FormatItem::Bold(true)]);
        assert_eq!(f[0].author, "Ani Wijaya");
        assert_eq!((f[0].start.offset, f[0].end.offset), (0, 5));
        // Reject at the caret: not bold any more, no change left.
        d.reject_change(Selection::caret(Position::new(0, 2)))
            .unwrap();
        assert!(d.changes().is_empty());
        assert_ne!(d.paragraph(0).runs[0].props.bold, Some(true));
        d.undo();
        // Accept: stays bold.
        d.accept_change(Selection::caret(Position::new(0, 2)))
            .unwrap();
        assert!(d.changes().is_empty());
        assert_eq!(d.paragraph(0).runs[0].props.bold, Some(true));
    }

    #[test]
    fn toggling_back_drops_the_change_and_own_insertions_are_plain() {
        let mut d = doc(&["hello world"]);
        d.toggle(sel(0, 0, 5), Toggle::Bold);
        d.toggle(sel(0, 0, 5), Toggle::Bold);
        assert!(formats(&d).is_empty(), "{:?}", d.changes());
        // Bold on text the user just typed (tracked as theirs): not a
        // formatting change.
        let c = d.insert_text(Selection::caret(Position::new(0, 11)), "!!", None);
        let _ = c;
        d.toggle(sel(0, 11, 13), Toggle::Bold);
        assert!(formats(&d).is_empty(), "{:?}", d.changes());
    }

    #[test]
    fn paragraph_formatting_is_recorded() {
        let mut d = doc(&["one", "two"]);
        d.set_align(Selection::caret(Position::new(1, 0)), Align::Center);
        let f = formats(&d);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].format, [FormatItem::Align(Align::Center)]);
        assert!(f[0].para);
        // Reject: back to left.
        d.reject_change(Selection::caret(Position::new(1, 1)))
            .unwrap();
        assert_eq!(d.paragraph(1).props.align, None);
        assert!(d.changes().is_empty());
        // Accept All keeps a style change.
        d.set_style(Selection::caret(Position::new(0, 0)), Some("Heading1"));
        assert_eq!(formats(&d).len(), 1);
        d.resolve_all(true, Selection::default());
        assert_eq!(d.paragraph(0).props.style.as_deref(), Some("Heading1"));
        assert!(!d.has_changes());
    }

    #[test]
    fn formatting_changes_save_and_read_back() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f.docx");
        let mut d = doc(&["hello world", "two"]);
        d.toggle(sel(0, 6, 11), Toggle::Italic);
        d.set_align(Selection::caret(Position::new(1, 0)), Align::Right);
        d.save_as(&path).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let mut zip = zip::ZipArchive::new(std::io::Cursor::new(&bytes[..])).unwrap();
        let mut body = String::new();
        std::io::Read::read_to_string(&mut zip.by_name("word/document.xml").unwrap(), &mut body)
            .unwrap();
        assert!(
            body.contains("<w:i/><w:rPrChange w:id=") && body.contains("><w:rPr/></w:rPrChange>"),
            "{body}"
        );
        assert!(
            body.contains("<w:jc w:val=\"right\"/><w:pPrChange w:id="),
            "{body}"
        );
        let back = Document::open(&path).unwrap();
        let f = formats(&back);
        assert_eq!(f.len(), 2, "{f:?}");
        assert_eq!(f[0].format, [FormatItem::Italic(true)]);
        assert_eq!(f[1].format, [FormatItem::Align(Align::Right)]);
        let mut rej = back.clone();
        rej.resolve_all(false, Selection::default());
        assert!(!rej.has_changes());
        assert_eq!(rej.paragraph(1).props.align, None);
        assert!(
            rej.paragraph(0)
                .runs
                .iter()
                .all(|r| r.props.italic != Some(true))
        );
    }
}
