//! Fields in the Docs view: Update Field (F9), Update Table, and following
//! hyperlinks with Ctrl+click.

use chrono::Offset as _;
use docs_engine::{FieldContext, LinkTarget, Position, Selection};
use gpui_kit::component::WindowExt as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::dialog::{DialogDescription, DialogFooter};
use gpui_kit::component::v_flex;
use gpui_kit::prelude::*;
use gpui_kit::*;
use kantor_ui::i18n::{t, tf};

use crate::DocsView;

/// The clock and time zone fields are updated with.
fn now() -> FieldContext {
    let now = chrono::Local::now();
    FieldContext {
        now: now.naive_local(),
        offset: now.offset().fix(),
    }
}

/// Addresses Word opens without asking first.
fn safe_scheme(url: &str) -> bool {
    let lower = url.trim().to_ascii_lowercase();
    ["http://", "https://", "mailto:"]
        .iter()
        .any(|s| lower.starts_with(s))
}

impl DocsView {
    /// Update the fields in the selection, or the field at the caret
    /// (Word's F9), as one undo step.
    pub fn update_fields_doc(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rev = self.doc.revision();
        let engine = self.engine.clone();
        let sel = self.doc.update_fields(self.sel, &now(), &mut |d| {
            docs_pdf::field_pages(d, &mut engine.borrow_mut())
        });
        if self.doc.revision() != rev {
            self.edited(sel, window, cx);
        } else {
            let here = if self.sel.is_empty() {
                self.doc.field_at(self.sel.head).is_some()
            } else {
                !self.doc.fields().is_empty()
            };
            self.status = if here {
                t("The fields are up to date")
            } else {
                t("No field to update here: put the caret in a field, or select the text with the fields")
            }
            .into();
            cx.notify();
        }
    }

    /// Rebuild the table of contents at the caret, or every one in the
    /// document (Word's Update Table), as one undo step.
    pub fn update_table_doc(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.doc.has_table_of_contents() {
            self.status = t("This document has no table of contents").into();
            cx.notify();
            return;
        }
        let rev = self.doc.revision();
        let engine = self.engine.clone();
        let sel = self
            .doc
            .update_tables_of_contents(self.sel, &now(), &mut |d| {
                docs_pdf::field_pages(d, &mut engine.borrow_mut())
            });
        if self.doc.revision() != rev {
            self.edited(sel, window, cx);
        } else {
            self.status = t("The table of contents is up to date").into();
            cx.notify();
        }
    }

    /// Follow the hyperlink at `pos` (Ctrl+click): a bookmark in the
    /// document moves the caret there; an address opens in its app, after
    /// a warning for anything but web and e-mail addresses (as Word does).
    /// Returns whether there was a link.
    pub(crate) fn follow_link(
        &mut self,
        pos: Position,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(target) = self.doc.link_at(pos) else {
            return false;
        };
        match target {
            LinkTarget::Bookmark(name) => match self.doc.bookmark_position(&name) {
                Some(at) => {
                    self.sel = Selection::caret(at);
                    self.goal_x = None;
                    self.scroll_to_caret();
                    cx.notify();
                }
                None => {
                    self.status = tf("The bookmark “{}” isn't in this document", &[&name]).into();
                    cx.notify();
                }
            },
            LinkTarget::Url(url) if safe_scheme(&url) => cx.open_url(&url),
            LinkTarget::Url(url) => confirm_open(url, window, cx),
        }
        true
    }
}

/// Ask before opening an address that isn't a web page or e-mail.
fn confirm_open(url: String, window: &mut Window, cx: &mut App) {
    if window.has_active_dialog(cx) {
        return;
    }
    let title: SharedString = t("Open this link?").into();
    let detail: SharedString = tf(
        "Links can be harmful to your computer and data. Open only links from sources you trust: {}",
        &[&url],
    )
    .into();
    window.open_dialog(cx, move |dialog, _, _| {
        let url = url.clone();
        dialog
            .w(px(460.))
            .title(title.clone())
            .child(v_flex().child(DialogDescription::new().child(detail.clone())))
            .footer(
                DialogFooter::new()
                    .child(
                        Button::new("link-cancel")
                            .label(t("Cancel"))
                            .ghost()
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .child(
                        Button::new("link-open")
                            .label(t("Open"))
                            .primary()
                            .on_click(move |_, window, cx| {
                                window.close_dialog(cx);
                                cx.open_url(&url);
                            }),
                    ),
            )
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[std::prelude::v1::test]
    fn web_and_mail_links_open_without_asking() {
        assert!(safe_scheme("https://kantor.paparanai.id"));
        assert!(safe_scheme("HTTP://example.com"));
        assert!(safe_scheme("mailto:halo@example.com"));
        assert!(!safe_scheme("file:///etc/passwd"));
        assert!(!safe_scheme("javascript:alert(1)"));
    }
}
