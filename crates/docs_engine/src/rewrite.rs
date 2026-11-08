//! Whole-paragraph text replacement, for "Rewrite / Translate selection".
//!
//! Paragraph properties (style, alignment, list, spacing) are kept. Run-level
//! formatting is simplified: each new paragraph takes the formatting of the
//! old paragraph's first text run, so bold words or hyperlinks inside the
//! paragraph are lost. Bookmark-like markers are kept at the paragraph start.
//! Paragraphs holding pictures, objects or page breaks are refused.

use crate::document::{Document, Kind, LINE_BREAK, PAGE_BREAK};
use crate::model::{OBJECT, Paragraph, Position, Run, Selection};

impl Document {
    /// The paragraphs a rewrite covers: every paragraph the selection touches
    /// (the caret's paragraph when nothing is selected). Returns their index
    /// range and plain text, line breaks as `\n`.
    pub fn rewrite_source(
        &self,
        sel: Selection,
    ) -> Result<(std::ops::Range<usize>, Vec<String>), String> {
        let sel = self.clamp_selection(sel);
        let (start, end) = sel.ordered();
        let range = start.para..end.para + 1;
        let mut texts = Vec::new();
        for p in &self.paragraphs[range.clone()] {
            check_rewritable(p)?;
            texts.push(p.text.replace(LINE_BREAK, "\n"));
        }
        Ok((range, texts))
    }

    /// Replace the text of paragraphs `range` with `texts` (one entry per
    /// paragraph; `\n` inside an entry is a line break) as one undo step.
    /// Extra entries become new paragraphs styled like the last one; missing
    /// ones remove paragraphs. Inside a table the count must match.
    /// Returns the selection covering the new text.
    pub fn replace_paragraph_texts(
        &mut self,
        range: std::ops::Range<usize>,
        texts: &[String],
    ) -> Result<Selection, String> {
        if range.is_empty() || range.end > self.paragraphs.len() {
            return Err("the paragraphs changed; select the text again".into());
        }
        if texts.is_empty() {
            return Err("nothing to insert".into());
        }
        let old = &self.paragraphs[range.clone()];
        for p in old {
            check_rewritable(p)?;
        }
        let in_table = old.iter().any(|p| p.cell.is_some());
        if in_table && texts.len() != old.len() {
            return Err(
                "inside a table the rewrite must keep the number of paragraphs; try again".into(),
            );
        }
        let new: Vec<Paragraph> = texts
            .iter()
            .enumerate()
            .map(|(i, text)| rebuild(&old[i.min(old.len() - 1)], text, i < old.len()))
            .collect();
        let before = Selection {
            anchor: Position::new(range.start, 0),
            head: Position::new(range.end - 1, old[old.len() - 1].text.len()),
        };
        let last = new.len() - 1;
        let after = Selection {
            anchor: Position::new(range.start, 0),
            head: Position::new(range.start + last, new[last].text.len()),
        };
        self.replace(
            range.start,
            range.len(),
            new,
            (before, after),
            Kind::Other,
            true,
        );
        Ok(after)
    }
}

fn check_rewritable(p: &Paragraph) -> Result<(), String> {
    if p.text.contains(PAGE_BREAK) {
        return Err("the selection has a page break; select text without it".into());
    }
    let objects = p
        .runs
        .iter()
        .any(|r| r.object.as_ref().is_some_and(|o| !o.is_marker()));
    if objects {
        return Err("the selection has pictures or objects; select text only".into());
    }
    Ok(())
}

/// `text` with `template`'s paragraph properties and first text run's
/// formatting. `keep_markers`: carry the template's markers over (only for
/// the paragraph that replaces it, so they aren't duplicated).
fn rebuild(template: &Paragraph, text: &str, keep_markers: bool) -> Paragraph {
    let props = template
        .runs
        .iter()
        .find(|r| r.object.is_none() && r.len > 0)
        .or_else(|| template.runs.iter().find(|r| r.object.is_none()))
        .map(|r| r.props.clone())
        .unwrap_or_default();
    let text: String = text
        .replace("\r\n", "\n")
        .chars()
        .filter(|&c| c != OBJECT && c != PAGE_BREAK)
        .map(|c| if c == '\n' { LINE_BREAK } else { c })
        .collect();
    let mut p = Paragraph {
        runs: vec![Run::text(text.len(), props)],
        text,
        props: template.props.clone(),
        cell: template.cell,
    };
    if keep_markers {
        let markers = template.markers_in(0..template.text.len());
        if !markers.is_empty() {
            p.insert_runs(0, markers);
        }
    }
    p
}

#[cfg(test)]
mod tests {
    use crate::model::{Paragraph, Position, RunProps, Selection};
    use crate::{Document, LINE_BREAK};

    fn doc() -> Document {
        let bold = RunProps {
            bold: Some(true),
            ..Default::default()
        };
        let mut p0 = Paragraph::styled("Halo dunia", bold).with_style("Heading1");
        p0.append(Paragraph::new(" lagi"));
        Document::from_paragraphs(vec![
            p0,
            Paragraph::new(format!("baris satu{LINE_BREAK}baris dua")),
            Paragraph::new("tetap"),
        ])
    }

    fn sel(a: (usize, usize), b: (usize, usize)) -> Selection {
        Selection {
            anchor: Position::new(a.0, a.1),
            head: Position::new(b.0, b.1),
        }
    }

    #[test]
    fn source_covers_touched_paragraphs() {
        let d = doc();
        let (range, texts) = d.rewrite_source(sel((0, 3), (1, 2))).unwrap();
        assert_eq!(range, 0..2);
        assert_eq!(texts, vec!["Halo dunia lagi", "baris satu\nbaris dua"]);
        let (range, _) = d.rewrite_source(sel((2, 1), (2, 1))).unwrap();
        assert_eq!(range, 2..3, "caret paragraph");
    }

    #[test]
    fn replace_keeps_styles_and_undoes_in_one_step() {
        let mut d = doc();
        let (range, _) = d.rewrite_source(sel((0, 0), (1, 1))).unwrap();
        let after = d
            .replace_paragraph_texts(
                range,
                &["Hello world again".into(), "line one\nline two".into()],
            )
            .unwrap();
        assert_eq!(d.paragraph(0).text, "Hello world again");
        assert_eq!(d.paragraph(0).props.style.as_deref(), Some("Heading1"));
        assert_eq!(d.paragraph(0).runs.len(), 1);
        assert_eq!(
            d.paragraph(0).runs[0].props.bold,
            Some(true),
            "first run's formatting"
        );
        assert_eq!(d.paragraph(1).text, format!("line one{LINE_BREAK}line two"));
        assert_eq!(d.paragraph(2).text, "tetap");
        assert_eq!(after.head, Position::new(1, d.paragraph(1).text.len()));

        d.undo().unwrap();
        assert_eq!(d.paragraph(0).text, "Halo dunia lagi");
        assert_eq!(
            d.paragraph(1).text,
            format!("baris satu{LINE_BREAK}baris dua")
        );
        assert!(!d.can_undo());
    }

    #[test]
    fn paragraph_count_may_change_outside_tables() {
        let mut d = doc();
        d.replace_paragraph_texts(0..2, &["Satu".into()]).unwrap();
        assert_eq!(d.len(), 2);
        assert_eq!(d.paragraph(1).text, "tetap");
        d.undo().unwrap();
        d.replace_paragraph_texts(2..3, &["a".into(), "b".into()])
            .unwrap();
        assert_eq!(d.len(), 4);
        assert_eq!(d.paragraph(3).text, "b");
        assert!(d.replace_paragraph_texts(9..10, &["x".into()]).is_err());
    }
}
