//! Hooks for programs driving Kantor (the MCP server, `kantor mcp`): read
//! the document, edit it the way the user's own commands do (undo, dirty
//! state, re-layout) and show what changed by selecting it.

use std::path::Path;

use anyhow::bail;
use docs_engine::{Document, Selection};
use gpui_kit::*;

use crate::DocsView;

impl DocsView {
    /// Read the document; `f` also gets the user's selection.
    pub fn agent_read<R>(&self, f: impl FnOnce(&Document, Selection) -> R) -> R {
        f(&self.doc, self.sel)
    }

    /// Edit the document. `f` gets the user's selection (to restore on
    /// undo) and returns the selection to show afterwards, which is
    /// revealed, with "Agent: {what}" in the status bar. Refused while an
    /// input method is composing text.
    pub fn agent_edit<R>(
        &mut self,
        what: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut Document, Selection) -> anyhow::Result<(R, Selection)>,
    ) -> anyhow::Result<R> {
        if self.marked.is_some() {
            bail!("the user is composing text in this document; try again in a moment");
        }
        let (r, sel) = f(&mut self.doc, self.sel)?;
        self.typing_props = None;
        self.edited(sel, window, cx);
        self.status = format!("Agent: {what}").into();
        Ok(r)
    }

    /// Save to the document's own file (`None`) or to `path` (the caller
    /// has checked overwriting). Returns the status text.
    pub fn agent_save(
        &mut self,
        path: Option<&Path>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<String> {
        let result = match path {
            Some(path) => self.doc.save_as(path),
            None if self.doc.needs_save_as() => {
                if self.doc.has_unsupported_content() {
                    bail!(
                        "this document has content Kantor can't keep ({}); save it under a new path",
                        self.doc.unsupported_content().join(", ")
                    )
                }
                if self.doc.is_imported() {
                    bail!("this document was imported (.rtf, .doc); give a .docx path")
                }
                bail!("this document has no file yet; give a path")
            }
            None => self.doc.save(),
        };
        let ok = result.is_ok();
        self.after_save(result, window, cx);
        if ok {
            Ok(self.status.to_string())
        } else {
            bail!("{}", self.status)
        }
    }
}
