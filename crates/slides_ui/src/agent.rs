//! Hooks for programs driving Kantor (the MCP server, `kantor mcp`): read
//! the presentation, edit it the way the user's own commands do (one undo
//! step, dirty state, redraw) and show what changed.

use std::path::Path;

use anyhow::bail;
use gpui_kit::*;
use slides_engine::{Focus, Presentation};

use crate::SlidesView;

impl SlidesView {
    /// Read the presentation; `f` also gets the slide on screen.
    pub fn agent_read<R>(&self, f: impl FnOnce(&Presentation, usize) -> R) -> R {
        f(&self.pres, self.current)
    }

    /// Edit the presentation as one undo step. `f` gets the slide on
    /// screen and returns where the change is (that slide and shape are
    /// shown and selected), with "Agent: {what}" in the status bar.
    /// Refused while the user drags a shape or composes text.
    pub fn agent_edit<R>(
        &mut self,
        what: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut Presentation, usize) -> anyhow::Result<(R, Focus)>,
    ) -> anyhow::Result<R> {
        if self.drag.is_some() || self.editing.as_ref().is_some_and(|e| e.marked.is_some()) {
            bail!("the user is editing this presentation; try again in a moment");
        }
        let current = self.current;
        let mut landed = None;
        let r = self.pres.transaction(Focus::slide(current), |p| {
            let (r, focus) = f(p, current)?;
            landed = Some(focus.clone());
            Ok((r, focus))
        })?;
        if let Some(focus) = landed {
            self.apply_focus(focus, cx);
        }
        self.after_edit(window, cx);
        self.status = format!("Agent: {what}").into();
        Ok(r)
    }

    /// Show slide `slide` (0-based), as if the user had clicked it.
    pub fn agent_go_to(&mut self, slide: usize, cx: &mut Context<Self>) {
        self.go_to(slide, cx);
    }

    /// Save to the presentation's own file (`None`) or to `path` (the
    /// caller has checked overwriting). Returns the status text.
    pub fn agent_save(
        &mut self,
        path: Option<&Path>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<String> {
        let result = match path {
            Some(path) => self.pres.save_as(path),
            None if self.pres.needs_save_as() => {
                if self.pres.has_unsupported_content() {
                    bail!(
                        "this presentation has content Kantor can't keep ({}); save it under a new path",
                        self.pres.unsupported_content().join(", ")
                    )
                }
                bail!("this presentation has no file yet; give a path")
            }
            None => self.pres.save(),
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
