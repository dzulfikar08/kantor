//! What a workbook's macros ask of the window: the other open workbooks
//! (`Workbooks`), lending one to a running macro and taking it back,
//! showing a workbook a macro opened or added in a new tab, and
//! `Workbook_BeforeClose` before a close goes on.

use super::doc::DocView;
use super::{Layout, Tab, Workspace, closing::Closing};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kantor_core::layout::PaneId;
use sheets_ui::{SheetsEvent, SheetsView};

impl Workspace {
    /// The macro events of the workbook in pane `id`.
    pub(super) fn sheets_macro_event(
        &mut self,
        id: PaneId,
        event: &SheetsEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            SheetsEvent::ListBooks(reply) => {
                let books = self.other_books(id, cx);
                if let Some(reply) = reply.take() {
                    let _ = reply.send(books);
                }
            }
            SheetsEvent::LendBook { id: other, reply } => {
                let book = match self.sheets_view(*other) {
                    Some(v) if *other != id => v.update(cx, |v, cx| v.lend_to_macro(window, cx)),
                    _ => None,
                };
                if let Some(reply) = reply.take() {
                    let _ = reply.send(book);
                }
            }
            SheetsEvent::ReturnBook {
                id: other,
                book,
                close,
            } => {
                if let (Some(v), Some(book)) = (self.sheets_view(*other), book.take()) {
                    v.update(cx, |v, cx| v.macro_returned(book, window, cx));
                    if *close {
                        self.remove_doc(*other, window, cx);
                    }
                }
            }
            SheetsEvent::OpenBook(book) => {
                if let Some(book) = book.take() {
                    let view =
                        DocView::Sheets(cx.new(|cx| SheetsView::from_book(book, window, cx)));
                    let pane = self.insert_doc(view, window, cx);
                    let tab = self.alloc_id();
                    self.tabs.push(Tab {
                        id: tab,
                        layout: Layout::Leaf(pane),
                        focused: pane,
                    });
                    self.activate(Some(self.tabs.len() - 1), window, cx);
                }
            }
            SheetsEvent::RetryClose => {
                if let Some(closing) = self.pending_close.take() {
                    self.request_close(closing, window, cx);
                }
            }
            SheetsEvent::CloseCancelled => self.pending_close = None,
            SheetsEvent::Close | SheetsEvent::Notice(_) | SheetsEvent::ExplainPls { .. } => {}
        }
    }

    fn sheets_view(&self, pane: PaneId) -> Option<Entity<SheetsView>> {
        match &self.docs.get(&pane)?.view {
            DocView::Sheets(v) => Some(v.clone()),
            _ => None,
        }
    }

    /// The workbooks open in the window other than pane `asking`'s.
    fn other_books(&self, asking: PaneId, cx: &App) -> Vec<sheets_script::OtherBook> {
        let mut out = Vec::new();
        for tab in &self.tabs {
            for pane in tab.layout.panes() {
                if pane == asking {
                    continue;
                }
                if let Some(v) = self.sheets_view(pane) {
                    let v = v.read(cx);
                    out.push(sheets_script::OtherBook {
                        id: pane,
                        name: v.doc_name(cx),
                        path: v.path(cx),
                    });
                }
            }
        }
        out
    }

    /// Before `closing` drops workbooks whose `Workbook_BeforeClose` should
    /// run: runs the first one's and waits (`RetryClose` closes again,
    /// `CloseCancelled` stops). `true`: the close waits.
    pub(super) fn before_close_macros(
        &mut self,
        closing: Closing,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        for pane in self.closing_panes(closing) {
            let Some(v) = self.sheets_view(pane) else {
                continue;
            };
            if !v.read(cx).needs_before_close() {
                continue;
            }
            if v.update(cx, |v, cx| v.fire_before_close(window, cx)) {
                self.pending_close = Some(closing);
                return true;
            }
        }
        false
    }
}
