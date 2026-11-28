//! `[[` completion: typing `[[` lists the vault's notes (best match first,
//! `Note#` lists its headings); Up/Down choose, Enter or Tab writes the
//! link with its `]]`, Escape closes the list.

use gpui_kit::component::v_flex;
use gpui_kit::prelude::*;
use gpui_kit::*;
use kantor_ui::{metrics::*, tokens};
use notes_engine::vault::{self as nv, Completion};
use notes_engine::{Merge, Selection};

use crate::NotesView;

/// Choices shown at most.
const MAX_ITEMS: usize = 8;

pub(crate) struct Popup {
    /// Where the query starts (just after `[[`).
    pub start: usize,
    pub items: Vec<Completion>,
    pub selected: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Key {
    Up,
    Down,
    Accept,
    Close,
}

impl NotesView {
    /// Open, update or close the list for where the caret is now.
    pub(crate) fn update_completion(&mut self, cx: &App) {
        let popup = self.completion_for_caret(cx);
        self.completion = match (popup, self.completion.take()) {
            (Some(mut new), Some(old)) if new.start == old.start => {
                // Keep the choice while it is still listed.
                if let Some(cur) = old.items.get(old.selected) {
                    new.selected = new.items.iter().position(|c| c == cur).unwrap_or(0);
                }
                Some(new)
            }
            (new, _) => new,
        };
    }

    fn completion_for_caret(&mut self, cx: &App) -> Option<Popup> {
        if !self.editable() {
            return None;
        }
        let sel = self.editor.selection();
        if !sel.is_empty() {
            return None;
        }
        let buf = self.editor.buffer();
        let line = buf.line_of(sel.head);
        // Not in code.
        if matches!(
            self.editor.parsed().lines.get(line).map(|l| &l.kind),
            Some(notes_engine::LineKind::Code { .. } | notes_engine::LineKind::CodeFence { .. })
        ) {
            return None;
        }
        let ls = buf.line_start(line);
        let before = &buf.text()[ls..sel.head];
        let (qs, q) = nv::completion_query(before)?;
        let q = q.to_owned();
        let rel = self.my_rel(cx);
        let ready = self.vault.as_ref().filter(|v| !v.read(cx).loading).cloned();
        let items = match ready {
            Some(v) => v.read(cx).index.complete(rel.as_deref(), &q, MAX_ITEMS),
            None => {
                let idx = self.folder_index()?;
                let name = rel
                    .as_deref()
                    .map(|r| r.rsplit('/').next().unwrap_or(r).to_owned());
                idx.complete(name.as_deref(), &q, MAX_ITEMS)
            }
        };
        (!items.is_empty()).then_some(Popup {
            start: ls + qs,
            items,
            selected: 0,
        })
    }

    /// A key while the list is open; returns whether it was used.
    pub(crate) fn completion_key(&mut self, key: Key, cx: &mut Context<Self>) -> bool {
        let Some(p) = &mut self.completion else {
            return false;
        };
        let n = p.items.len().max(1);
        match key {
            Key::Up => p.selected = (p.selected + n - 1) % n,
            Key::Down => p.selected = (p.selected + 1) % n,
            Key::Close => self.completion = None,
            Key::Accept => {
                let i = p.selected;
                self.accept_completion(i, cx);
                return true;
            }
        }
        cx.notify();
        true
    }

    /// Write choice `i`: the query becomes the name, then `]]`.
    pub(crate) fn accept_completion(&mut self, i: usize, cx: &mut Context<Self>) {
        let Some(p) = self.completion.take() else {
            return;
        };
        let Some(c) = p.items.get(i) else {
            return;
        };
        let head = self.editor.selection().head;
        if head < p.start {
            return;
        }
        let rest = &self.editor.text()[head..];
        let closed = rest.starts_with("]]");
        let text = if closed {
            c.insert.clone()
        } else {
            format!("{}]]", c.insert)
        };
        let caret = p.start + c.insert.len() + 2;
        self.editor.edit(
            vec![(p.start..head, text)],
            Some(Selection::caret(caret)),
            Merge::No,
        );
        self.edited(cx);
        self.completion = None;
    }

    /// The list under the caret (in the editor area's coordinates).
    pub(crate) fn render_completion(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let p = self.completion.as_ref()?;
        let c = self.caret_pos()?;
        let tok = tokens(cx);
        let o = self.origin();
        let vp = self.viewport.get();
        let x = o.x - vp.origin.x + c.at.x;
        let y = o.y - vp.origin.y + c.at.y + c.height + px(4.);
        let mut list = v_flex()
            .id("notes-completion")
            .absolute()
            .left(x.max(px(4.)))
            .top(y)
            .w(px(300.))
            .py(SPACE_2)
            .rounded(RADIUS_MD)
            .border_1()
            .border_color(tok.border)
            .bg(tok.elevated)
            .shadow_md()
            .text_size(FONT_BODY)
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation());
        for (i, item) in p.items.iter().enumerate() {
            let on = i == p.selected;
            list = list.child(
                v_flex()
                    .id(("notes-completion-item", i))
                    .px(SPACE_4)
                    .py(SPACE_1)
                    .cursor_pointer()
                    .when(on, |d| d.bg(tok.selected))
                    .hover(|s| s.bg(tok.hover))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.accept_completion(i, cx);
                        }),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_color(tok.fg)
                            .child(item.label.clone()),
                    )
                    .when(item.detail != format!("{}.md", item.label), |d| {
                        d.child(
                            div()
                                .truncate()
                                .text_size(FONT_CAPTION)
                                .text_color(tok.fg_dim)
                                .child(item.detail.clone()),
                        )
                    }),
            );
        }
        Some(list.into_any_element())
    }
}
