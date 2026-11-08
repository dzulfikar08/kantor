//! Slide-level edits for programs driving Kantor (the MCP server): several
//! edits as one undo step, a new slide with its title and bullets, and
//! replacing a shape's text.

use anyhow::{Context as _, bail};
use docs_engine::{LINE_BREAK, Position, Selection};

use crate::edit::ShapeRef;
use crate::history::Focus;
use crate::presentation::Presentation;
use crate::text_ops;

/// One bullet of a new slide's body.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BodyLine {
    pub text: String,
    /// Outline level, 0–8.
    pub level: u8,
}

/// Text a shape can hold: no control characters but tabs and new lines.
fn clean(text: &str) -> String {
    text.replace("\r\n", "\n")
        .replace('\r', "\n")
        .chars()
        .filter(|&c| c == '\t' || c == '\n' || !c.is_control())
        .collect()
}

impl Presentation {
    /// Run `f` as one undo step: whatever it records is folded into a
    /// single command (`before` is restored on undo, the focus `f` returns
    /// on redo). On error the presentation is put back as it was.
    pub fn transaction<R>(
        &mut self,
        before: Focus,
        f: impl FnOnce(&mut Self) -> anyhow::Result<(R, Focus)>,
    ) -> anyhow::Result<R> {
        self.history.seal();
        let depth = self.history.depth();
        let snapshot = self.slides.clone();
        match f(self) {
            Ok((r, after)) => {
                let now = self.slides.clone();
                self.history
                    .squash_since(depth, snapshot, now, (before, after));
                Ok(r)
            }
            Err(e) => {
                self.slides = snapshot;
                self.history.truncate(depth);
                Err(e)
            }
        }
    }

    /// Index of the shape whose id (`p:cNvPr/@id`) is `id` on `slide`.
    pub fn shape_index(&self, slide: usize, id: u32) -> Option<usize> {
        self.slides
            .get(slide)?
            .shapes
            .iter()
            .position(|s| s.id == id)
    }

    /// Replace all the text of a shape (`\n` starts a paragraph); the
    /// formatting of its first character is kept. One undo step.
    pub fn set_shape_text(&mut self, r: ShapeRef, text: &str) -> anyhow::Result<()> {
        let shape = self
            .slides
            .get(r.0)
            .and_then(|s| s.shapes.get(r.1))
            .context("no such shape")?;
        if !shape.text_editable() {
            match shape.text.as_ref().and_then(|t| t.locked.as_deref()) {
                Some(why) => bail!("the text of \"{}\" can't be edited: {why}", shape.name),
                None => bail!("\"{}\" doesn't hold text", shape.name),
            }
        }
        let text = clean(text);
        self.transaction(Focus::shape(r.0, r.1), |p| {
            let paras = p.text_paras(r).context("no text")?;
            let all = Selection {
                anchor: Position::new(0, 0),
                head: text_ops::end(paras),
            };
            p.insert_text(r, all, &text, None)?;
            Ok(((), Focus::shape(r.0, r.1)))
        })
    }

    /// Add a slide after `after` (`None`: first) with layout `layout`
    /// (default: as [`Self::new_slide`] picks), filling its title
    /// placeholder with `title` and its first other placeholder with
    /// `body`, one paragraph per line. One undo step; returns its index.
    pub fn add_slide_with_text(
        &mut self,
        after: Option<usize>,
        layout: Option<usize>,
        title: Option<&str>,
        body: &[BodyLine],
    ) -> anyhow::Result<usize> {
        self.transaction(Focus::slide(after.unwrap_or(0)), |p| {
            let i = p.new_slide_with_layout(after, layout)?;
            let shapes = &p.slides[i].shapes;
            let placeholder = |title: bool| {
                shapes
                    .iter()
                    .position(|s| s.ph.as_ref().is_some_and(|ph| ph.is_title() == title))
            };
            let (title_ix, body_ix) = (placeholder(true), placeholder(false));
            if let Some(t) = title.filter(|t| !t.is_empty()) {
                let ix = title_ix.context("this layout has no title placeholder")?;
                p.set_shape_text((i, ix), &t.replace(['\r', '\n'], " "))?;
            }
            if !body.is_empty() {
                let ix = body_ix.context("this layout has no placeholder for body text")?;
                let text: Vec<String> = body
                    .iter()
                    .map(|l| clean(&l.text).replace('\n', &LINE_BREAK.to_string()))
                    .collect();
                p.set_shape_text((i, ix), &text.join("\n"))?;
                for (n, line) in body.iter().enumerate() {
                    if line.level > 0 {
                        let sel = Selection::caret(Position::new(n, 0));
                        p.change_level((i, ix), sel, line.level.min(8) as i8)?;
                    }
                }
            }
            Ok((i, Focus::slide(i)))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(text: &str, level: u8) -> BodyLine {
        BodyLine {
            text: text.into(),
            level,
        }
    }

    fn shape_texts(p: &Presentation, slide: usize) -> Vec<String> {
        p.slide(slide)
            .shapes
            .iter()
            .filter_map(|s| s.text.as_ref().map(|t| t.plain_text()))
            .collect()
    }

    #[test]
    fn a_new_slide_with_text_is_one_undo_step() {
        let mut p = Presentation::new();
        let n = p.len();
        let i = p
            .add_slide_with_text(
                Some(n - 1),
                None,
                Some("Plan"),
                &[line("First", 0), line("Detail", 1)],
            )
            .unwrap();
        assert_eq!(i, n);
        let texts = shape_texts(&p, i);
        assert!(texts.iter().any(|t| t == "Plan"), "{texts:?}");
        assert!(texts.iter().any(|t| t.starts_with("First")), "{texts:?}");
        let body = p
            .slide(i)
            .shapes
            .iter()
            .find_map(|s| s.text.as_ref().filter(|t| t.paras.len() == 2))
            .unwrap();
        assert_eq!(body.paras[1].props.level, 1);
        assert!(p.undo().is_some());
        assert_eq!(p.len(), n);
        assert!(p.redo().is_some());
        assert_eq!(p.len(), n + 1);
    }

    #[test]
    fn sets_shape_text_and_rolls_back_errors() {
        let mut p = Presentation::new();
        let id = p.slide(0).shapes[0].id;
        let ix = p.shape_index(0, id).unwrap();
        p.set_shape_text((0, ix), "Hello\nWorld").unwrap();
        assert_eq!(p.slide(0).shapes[ix].text.as_ref().unwrap().paras.len(), 2);
        assert!(p.set_shape_text((0, 99), "x").is_err());
        let before = p.len();
        let r = p.add_slide_with_text(Some(0), Some(999), Some("x"), &[]);
        assert!(r.is_err());
        assert_eq!(p.len(), before);
        assert!(p.undo().is_some());
        assert!(shape_texts(&p, 0).iter().all(|t| t.is_empty()));
    }
}
