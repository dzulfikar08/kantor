//! Undo/redo. Every edit is one command holding the slide list before and
//! after (slides are `Arc`s, so a snapshot is cheap and unchanged slides
//! are shared). Typing, deleting, IME composition and arrow-key nudges of
//! one shape coalesce into one step.

use std::sync::Arc;

use docs_engine::Selection;

use crate::presentation::Slide;

/// Where the user was: the slide, the selected shape and the text selection.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Focus {
    pub slide: usize,
    pub shape: Option<usize>,
    pub sel: Option<Selection>,
}

impl Focus {
    pub fn slide(slide: usize) -> Self {
        Self {
            slide,
            ..Self::default()
        }
    }

    pub fn shape(slide: usize, shape: usize) -> Self {
        Self {
            slide,
            shape: Some(shape),
            sel: None,
        }
    }

    pub fn text(slide: usize, shape: usize, sel: Selection) -> Self {
        Self {
            slide,
            shape: Some(shape),
            sel: Some(sel),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Typing,
    Deleting,
    Composing,
    Nudge,
    /// Typing in a slide's speaker notes.
    Notes,
    Other,
}

#[derive(Debug)]
pub(crate) struct Command {
    id: u64,
    pub before: Vec<Arc<Slide>>,
    pub after: Vec<Arc<Slide>>,
    pub focus_before: Focus,
    pub focus_after: Focus,
    pub kind: Kind,
    /// The shape a coalescing command edits.
    pub target: Option<(usize, usize)>,
}

impl Command {
    pub fn new(
        before: Vec<Arc<Slide>>,
        after: Vec<Arc<Slide>>,
        focus_before: Focus,
        focus_after: Focus,
        kind: Kind,
        target: Option<(usize, usize)>,
    ) -> Self {
        Self {
            id: 0,
            before,
            after,
            focus_before,
            focus_after,
            kind,
            target,
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct History {
    undo: Vec<Command>,
    redo: Vec<Command>,
    next_id: u64,
    saved: Option<u64>,
    sealed: bool,
}

impl History {
    fn top(&self) -> Option<u64> {
        self.undo.last().map(|c| c.id)
    }

    pub fn is_dirty(&self) -> bool {
        self.top() != self.saved
    }

    pub fn mark_saved(&mut self) {
        self.saved = self.top();
        self.sealed = true;
    }

    /// Don't merge the next command into the current top one.
    pub fn seal(&mut self) {
        self.sealed = true;
    }

    pub fn push(&mut self, mut cmd: Command, word_boundary: bool) {
        self.redo.clear();
        let sealed = std::mem::take(&mut self.sealed);
        if let Some(last) = self.undo.last_mut()
            && !sealed
            && cmd.kind != Kind::Other
            && cmd.kind == last.kind
            && cmd.target.is_some()
            && cmd.target == last.target
            && Some(last.id) != self.saved
            && match cmd.kind {
                Kind::Composing | Kind::Nudge | Kind::Notes => true,
                _ => !word_boundary && last.focus_after == cmd.focus_before,
            }
        {
            last.after = cmd.after;
            last.focus_after = cmd.focus_after;
            return;
        }
        self.next_id += 1;
        cmd.id = self.next_id;
        self.undo.push(cmd);
    }

    pub fn undo(&mut self) -> Option<&Command> {
        let cmd = self.undo.pop()?;
        self.sealed = true;
        self.redo.push(cmd);
        self.redo.last()
    }

    pub fn redo(&mut self) -> Option<&Command> {
        let cmd = self.redo.pop()?;
        self.sealed = true;
        self.undo.push(cmd);
        self.undo.last()
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }
}
