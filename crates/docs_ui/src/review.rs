//! Review in the Docs view: comment cards in a markup column to the right
//! of the pages (each beside the text it's on, joined by a line; the text
//! is highlighted), adding, editing and deleting comments, and the Review
//! menu: accept or reject the tracked change at the caret, all of them,
//! and go to the next or previous change. Tracked insertions and deletions
//! are coloured by author on the page itself (`docs_pdf::build`).

use docs_engine::Selection;
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::input::{InputEvent, Textarea, TextareaState};
use gpui_kit::component::{
    Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    menu::{DropdownMenu as _, PopupMenu, PopupMenuItem},
    v_flex,
};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kantor_ui::tokens::Elevation;
use kantor_ui::{metrics::*, tokens};

use crate::DocsView;

/// Width of the markup column, screen px.
pub(crate) const MARKUP_W: f32 = 264.0;
/// Card width inside it.
const CARD_W: f32 = 232.0;

/// A comment being written or edited.
pub(crate) struct CommentEdit {
    id: String,
    input: Entity<TextareaState>,
    /// Added just now: cancelling removes it again.
    new: bool,
    _sub: Subscription,
}

/// A comment card placed in the markup column.
#[derive(Clone, Debug)]
pub(crate) struct Card {
    pub id: String,
    /// The anchor on the page, window coordinates.
    pub anchor: Point<Pixels>,
    /// The card's top-left corner, window coordinates.
    pub at: Point<Pixels>,
    pub active: bool,
}

/// A short date for a card: `12 Nov 2026` from Word's ISO date.
fn short_date(iso: &str) -> String {
    const M: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let d: Vec<&str> = iso.get(..10).unwrap_or("").split('-').collect();
    match (
        d.first(),
        d.get(1).and_then(|m| m.parse::<usize>().ok()),
        d.get(2),
    ) {
        (Some(y), Some(m), Some(day)) if (1..=12).contains(&m) => {
            format!("{} {} {y}", day.trim_start_matches('0'), M[m - 1])
        }
        _ => String::new(),
    }
}

impl DocsView {
    /// Whether the markup column shows (the document has comments).
    pub(crate) fn markup_visible(&self) -> bool {
        self.comments_visible && (!self.doc.comments().is_empty() || self.comment_edit.is_some())
    }

    pub fn toggle_comments_doc(&mut self, cx: &mut Context<Self>) {
        self.comments_visible = !self.comments_visible;
        self.clamp_scroll();
        cx.notify();
    }

    /// Comment on the selection (or the word at the caret) and start
    /// writing it.
    pub fn new_comment_doc(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.save_comment_edit(window, cx);
        let author = kantor_core::settings::Settings::load().author_name();
        let Some(id) = self.doc.add_comment(self.sel, "", &author) else {
            return;
        };
        self.comments_visible = true;
        let sel = self.sel;
        self.edited(sel, window, cx);
        self.edit_comment(id, true, window, cx);
    }

    fn edit_comment(&mut self, id: String, new: bool, window: &mut Window, cx: &mut Context<Self>) {
        let text = self
            .doc
            .comments()
            .iter()
            .find(|c| c.id == id)
            .map(|c| c.text.clone())
            .unwrap_or_default();
        let input = cx.new(|cx| {
            let mut s = TextareaState::new(window, cx)
                .auto_grow(2, 8)
                .placeholder("Write a comment");
            s.set_value(text, window, cx);
            s
        });
        let sub = cx.subscribe_in(&input, window, |this, _, ev: &InputEvent, window, cx| {
            if let InputEvent::PressEnter {
                secondary: true, ..
            } = ev
            {
                this.save_comment_edit(window, cx);
                this.focus(window, cx);
            }
        });
        input.update(cx, |s, cx| s.focus(window, cx));
        self.comment_edit = Some(CommentEdit {
            id,
            input,
            new,
            _sub: sub,
        });
        cx.notify();
    }

    /// Keep what's typed in the open comment editor.
    pub(crate) fn save_comment_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(edit) = self.comment_edit.take() else {
            return;
        };
        let text = edit.input.read(cx).value().trim_end().to_string();
        if text.is_empty() && edit.new {
            // An empty new comment isn't kept.
            self.doc.undo();
            let sel = self.sel;
            self.edited(sel, window, cx);
            return;
        }
        if self.doc.set_comment_text(&edit.id, &text, self.sel) {
            let sel = self.sel;
            self.edited(sel, window, cx);
        }
        cx.notify();
    }

    fn cancel_comment_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(edit) = self.comment_edit.take() else {
            return;
        };
        if edit.new {
            self.doc.undo();
            let sel = self.sel;
            self.edited(sel, window, cx);
        }
        self.focus(window, cx);
        cx.notify();
    }

    fn delete_comment_doc(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.comment_edit = None;
        if self.doc.delete_comment(id, self.sel) {
            let sel = self.sel;
            self.edited(sel, window, cx);
            self.status = "Comment deleted".into();
        }
        cx.notify();
    }

    fn go_to_comment(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        if let Some((_, a)) = self
            .doc
            .comment_anchors()
            .into_iter()
            .find(|(i, _)| i == id)
        {
            self.sel = Selection {
                anchor: a.start,
                head: a.end,
            };
            self.reveal_caret.set(true);
            self.focus(window, cx);
            cx.notify();
        }
    }

    // ── tracked changes ────────────────────────────────────────────────────

    /// Review > Track Changes: record the user's own edits as insertions
    /// and deletions (Word's Ctrl+Shift+E).
    pub fn toggle_track_changes_doc(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let author = kantor_core::settings::Settings::load().author_name();
        self.doc.set_author(&author);
        let on = !self.doc.track_changes();
        self.doc.set_track_changes(on, self.sel);
        self.status = if on {
            format!("Track Changes on: your edits are marked as {author}'s").into()
        } else {
            "Track Changes off".into()
        };
        let sel = self.sel;
        self.edited(sel, window, cx);
    }

    pub fn accept_change_doc(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.resolve_change(true, window, cx);
    }

    pub fn reject_change_doc(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.resolve_change(false, window, cx);
    }

    fn resolve_change(&mut self, accept: bool, window: &mut Window, cx: &mut Context<Self>) {
        let done = if accept {
            self.doc.accept_change(self.sel)
        } else {
            self.doc.reject_change(self.sel)
        };
        match done {
            Some(sel) => {
                self.edited(sel, window, cx);
                // Move on to the next change, as Word does.
                self.go_to_change(true, cx);
                self.status = self.changes_status(if accept { "Accepted" } else { "Rejected" });
            }
            None => self.status = "No tracked change at the caret".into(),
        }
        cx.notify();
    }

    pub fn resolve_all_doc(&mut self, accept: bool, window: &mut Window, cx: &mut Context<Self>) {
        let n = self.doc.changes().len() + self.doc.other_changes();
        if n == 0 {
            self.status = "This document has no tracked changes".into();
            cx.notify();
            return;
        }
        let sel = self.doc.resolve_all(accept, self.sel);
        self.edited(sel, window, cx);
        self.status = format!(
            "{} {n} tracked change{}",
            if accept { "Accepted" } else { "Rejected" },
            if n == 1 { "" } else { "s" }
        )
        .into();
        cx.notify();
    }

    fn changes_status(&self, verb: &str) -> SharedString {
        let left = self.doc.changes().len() + self.doc.other_changes();
        match left {
            0 => format!("{verb}: no tracked changes left").into(),
            1 => format!("{verb}: 1 tracked change left").into(),
            n => format!("{verb}: {n} tracked changes left").into(),
        }
    }

    /// Select the next (or previous) tracked change after the caret.
    pub fn go_to_change(&mut self, forward: bool, cx: &mut Context<Self>) -> bool {
        let changes = self.doc.changes();
        let (a, b) = self.sel.ordered();
        let found = if forward {
            changes
                .iter()
                .find(|c| c.start > a || (c.start == a && c.end > b))
                .or(changes.first())
        } else {
            changes
                .iter()
                .rev()
                .find(|c| c.start < a)
                .or(changes.last())
        };
        match found {
            Some(c) => {
                self.sel = Selection {
                    anchor: c.start,
                    head: c.end,
                };
                self.reveal_caret.set(true);
                cx.notify();
                true
            }
            None => false,
        }
    }

    pub fn next_change_doc(&mut self, forward: bool, cx: &mut Context<Self>) {
        if !self.go_to_change(forward, cx) {
            self.status = "This document has no tracked changes".into();
            cx.notify();
        }
    }

    // ── cards ──────────────────────────────────────────────────────────────

    /// Comment cards for the comments on visible pages, stacked so they
    /// don't overlap.
    pub(crate) fn comment_cards(&self) -> Vec<Card> {
        if !self.markup_visible() {
            return Vec::new();
        }
        let g = self.geometry();
        let paged = &self.laid.paged;
        let visible = g.visible_pages(paged.pages.len());
        let (sa, sb) = self.sel.ordered();
        let mut cards: Vec<(f32, Card, f32)> = Vec::new();
        for (id, a) in self.doc.comment_anchors() {
            let Some(c) = paged.caret(a.start.para, a.start.offset) else {
                continue;
            };
            if !visible.contains(&c.page) {
                continue;
            }
            let anchor = g.page_to_screen(c.page, c.x, c.top + c.height * 0.85);
            let (pw, _) = g.page_size(c.page);
            let right = g.page_to_screen(c.page, pw, c.top);
            let at = point(right.x + px(16.), g.page_to_screen(c.page, 0., c.top).y);
            let active = (a.start <= sb && a.end >= sa)
                || self.comment_edit.as_ref().is_some_and(|e| e.id == id);
            let text = self
                .doc
                .comments()
                .iter()
                .find(|x| x.id == id)
                .map_or(0, |x| x.text.chars().count());
            let editing = self.comment_edit.as_ref().is_some_and(|e| e.id == id);
            // Rough height: header, ~32 characters a line, padding.
            let lines = (text as f32 / 32.0).ceil().max(1.0);
            let h = if editing { 150.0 } else { 46.0 + lines * 18.0 };
            cards.push((
                f32::from(at.y),
                Card {
                    id,
                    anchor,
                    at,
                    active,
                },
                h,
            ));
        }
        cards.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut bottom = f32::MIN;
        cards
            .into_iter()
            .map(|(y, mut card, h)| {
                let top = y.max(bottom + 8.0);
                card.at.y = px(top);
                bottom = top + h;
                card
            })
            .collect()
    }

    /// The comment cards over the page area.
    pub(crate) fn render_cards(&self, cards: &[Card], cx: &mut Context<Self>) -> Vec<AnyElement> {
        let t = tokens(cx);
        let origin = self.viewport.get().origin;
        let view = cx.entity().downgrade();
        cards
            .iter()
            .filter_map(|card| {
                let c = self.doc.comments().iter().find(|c| c.id == card.id)?;
                let id = card.id.clone();
                let editing = self.comment_edit.as_ref().filter(|e| e.id == card.id);
                let header = h_flex()
                    .gap(SPACE_2)
                    .child(
                        div()
                            .flex_none()
                            .size(px(20.))
                            .rounded_full()
                            .bg(t.accent_soft)
                            .text_color(t.accent)
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_size(FONT_SMALL)
                            .child(if c.initials.is_empty() {
                                c.author
                                    .chars()
                                    .next()
                                    .map(String::from)
                                    .unwrap_or_default()
                            } else {
                                c.initials.clone()
                            }),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(div().text_color(t.fg).truncate().child(c.author.clone()))
                            .child(
                                div()
                                    .text_size(FONT_SMALL)
                                    .text_color(t.fg_dim)
                                    .child(short_date(&c.date)),
                            ),
                    )
                    .when(editing.is_none(), |h| {
                        let (v1, v2, i1, i2) = (view.clone(), view.clone(), id.clone(), id.clone());
                        h.child(
                            Button::new(SharedString::from(format!("comment-edit-{id}")))
                                .ghost()
                                .xsmall()
                                .icon(Lucide::SquarePen)
                                .tooltip("Edit comment")
                                .on_click(move |_, window, cx| {
                                    let _ = v1.update(cx, |this, cx| {
                                        this.save_comment_edit(window, cx);
                                        this.edit_comment(i1.clone(), false, window, cx)
                                    });
                                }),
                        )
                        .child(
                            Button::new(SharedString::from(format!("comment-delete-{id}")))
                                .ghost()
                                .xsmall()
                                .icon(Lucide::Trash)
                                .tooltip("Delete comment")
                                .on_click(move |_, window, cx| {
                                    let _ = v2.update(cx, |this, cx| {
                                        this.delete_comment_doc(&i2, window, cx)
                                    });
                                }),
                        )
                    });
                let body: AnyElement = match editing {
                    Some(e) => {
                        let (v1, v2) = (view.clone(), view.clone());
                        v_flex()
                            .gap(SPACE_2)
                            .child(Textarea::new(&e.input).small())
                            .child(
                                h_flex()
                                    .gap(SPACE_2)
                                    .child(div().flex_1())
                                    .child(
                                        Button::new("comment-cancel")
                                            .ghost()
                                            .xsmall()
                                            .label("Cancel")
                                            .on_click(move |_, window, cx| {
                                                let _ = v1.update(cx, |this, cx| {
                                                    this.cancel_comment_edit(window, cx)
                                                });
                                            }),
                                    )
                                    .child(
                                        Button::new("comment-save")
                                            .primary()
                                            .xsmall()
                                            .label("Comment")
                                            .tooltip("Ctrl+Enter")
                                            .on_click(move |_, window, cx| {
                                                let _ = v2.update(cx, |this, cx| {
                                                    this.save_comment_edit(window, cx);
                                                    this.focus(window, cx);
                                                });
                                            }),
                                    ),
                            )
                            .into_any_element()
                    }
                    None => div()
                        .text_color(t.fg)
                        .whitespace_normal()
                        .child(c.text.clone())
                        .into_any_element(),
                };
                let v = view.clone();
                let go = id.clone();
                Some(
                    v_flex()
                        .id(SharedString::from(format!("comment-card-{id}")))
                        .absolute()
                        .left(card.at.x - origin.x)
                        .top(card.at.y - origin.y)
                        .w(px(CARD_W))
                        .gap(SPACE_2)
                        .p(SPACE_3)
                        .rounded(RADIUS_LG)
                        .bg(t.elevated)
                        .border_1()
                        .border_color(if card.active { t.accent } else { t.border })
                        .when(card.active, |d| d.shadow(t.shadow(Elevation::Popover)))
                        .text_size(FONT_SMALL)
                        .cursor_pointer()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(move |_, window, cx| {
                            let _ = v.update(cx, |this, cx| {
                                if this.comment_edit.is_none() {
                                    this.go_to_comment(&go, window, cx)
                                }
                            });
                        })
                        .child(header)
                        .child(body)
                        .into_any_element(),
                )
            })
            .collect()
    }
}

/// The toolbar's Review menu.
pub(crate) fn review_button(view: &DocsView, cx: &mut Context<DocsView>) -> impl IntoElement {
    let v = cx.entity().downgrade();
    let changes = view.doc.changes().len() + view.doc.other_changes();
    let comments = view.doc.comments().len();
    let track = view.doc.track_changes();
    let tip = match (changes, comments) {
        (0, 0) => "Review: comments and tracked changes".to_owned(),
        (c, 0) => format!(
            "Review: {c} tracked change{}",
            if c == 1 { "" } else { "s" }
        ),
        (0, n) => format!("Review: {n} comment{}", if n == 1 { "" } else { "s" }),
        (c, n) => format!("Review: {c} tracked changes, {n} comments"),
    };
    let tip = if track {
        format!("{tip} (Track Changes is on)")
    } else {
        tip
    };
    Button::new("docs-review")
        .ghost()
        .xsmall()
        .icon(if track {
            Lucide::FileDiff
        } else {
            Lucide::MessageSquare
        })
        .dropdown_caret(true)
        .tooltip(tip)
        .dropdown_menu(move |menu, _, _| review_items(menu, &v, track))
}

fn review_items(menu: PopupMenu, view: &WeakEntity<DocsView>, track: bool) -> PopupMenu {
    type Run = fn(&mut DocsView, &mut Window, &mut Context<DocsView>);
    let item = |menu: PopupMenu, label: &'static str, icon: Option<Lucide>, run: Run| {
        let v = view.clone();
        let mut it = PopupMenuItem::new(label).on_click(move |_, window, cx| {
            let _ = v.update(cx, |this, cx| run(this, window, cx));
        });
        if let Some(i) = icon {
            it = it.icon(i);
        }
        menu.item(it)
    };
    let v = view.clone();
    let menu = menu
        .item(
            PopupMenuItem::new("Track Changes  Ctrl+Shift+E")
                .checked(track)
                .on_click(move |_, window, cx| {
                    let _ = v.update(cx, |this, cx| this.toggle_track_changes_doc(window, cx));
                }),
        )
        .separator();
    let menu = item(
        menu,
        "New comment  Ctrl+Alt+M",
        Some(Lucide::MessageSquarePlus),
        |v, w, cx| v.new_comment_doc(w, cx),
    );
    let menu = item(menu, "Show or hide comments", None, |v, _, cx| {
        v.toggle_comments_doc(cx)
    })
    .separator();
    let menu = item(menu, "Accept change", Some(Lucide::Check), |v, w, cx| {
        v.accept_change_doc(w, cx)
    });
    let menu = item(menu, "Reject change", Some(Lucide::X), |v, w, cx| {
        v.reject_change_doc(w, cx)
    });
    let menu = item(
        menu,
        "Next change",
        Some(Lucide::ChevronDown),
        |v, _, cx| v.next_change_doc(true, cx),
    );
    let menu = item(
        menu,
        "Previous change",
        Some(Lucide::ChevronUp),
        |v, _, cx| v.next_change_doc(false, cx),
    )
    .separator();
    let menu = item(
        menu,
        "Accept all changes",
        Some(Lucide::CheckCheck),
        |v, w, cx| v.resolve_all_doc(true, w, cx),
    );
    item(menu, "Reject all changes", None, |v, w, cx| {
        v.resolve_all_doc(false, w, cx)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use docs_engine::Position;

    #[std::prelude::v1::test]
    fn short_dates() {
        assert_eq!(short_date("2026-11-03T09:00:00Z"), "3 Nov 2026");
        assert_eq!(short_date(""), "");
    }

    #[std::prelude::v1::test]
    fn position_is_ordered() {
        assert!(Position::new(1, 0) > Position::new(0, 9));
    }
}
