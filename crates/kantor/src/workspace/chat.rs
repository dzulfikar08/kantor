//! Paparan AI's chat panel (`crate::ai::chat`) docked on the right of the
//! window, beside the inspector. The panel lives on while hidden, so a job
//! keeps running and its conversation stays.

use gpui_kit::*;

use super::{Drag, Workspace};
use crate::actions::ToggleChat;
use crate::ai::chat::{ChatEvent, ChatPanel};

/// The chat panel's width limits.
const MIN_WIDTH: f32 = 300.;
const MAX_WIDTH: f32 = 760.;

/// Width of the invisible drag strip on the panel's edge.
const HANDLE: Pixels = px(6.);

pub(super) struct ChatDock {
    panel: Entity<ChatPanel>,
    _events: Subscription,
}

pub(super) fn clamp_width(w: f32) -> f32 {
    w.clamp(MIN_WIDTH, MAX_WIDTH)
}

impl Workspace {
    /// The panel, made the first time it's needed.
    fn chat_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Entity<ChatPanel> {
        if let Some(dock) = &self.chat {
            return dock.panel.clone();
        }
        let panel = cx.new(|cx| ChatPanel::new(window, cx));
        let events = cx.subscribe_in(
            &panel,
            window,
            |ws, _, ev: &ChatEvent, window, cx| match ev {
                ChatEvent::Open(path) => ws.open_paths(None, vec![path.clone()], window, cx),
                ChatEvent::Close => ws.set_chat_open(false, window, cx),
                ChatEvent::OpenSettings => {
                    ws.open_ai_settings(&kantor_ui::ai::OpenAiSettings, window, cx)
                }
            },
        );
        self.chat = Some(ChatDock {
            panel: panel.clone(),
            _events: events,
        });
        panel
    }

    /// Show the panel at start when it was open last time.
    pub(super) fn init_chat(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.ui.chat_open {
            self.chat_panel(window, cx);
        }
    }

    pub(super) fn toggle_chat(
        &mut self,
        _: &ToggleChat,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let open = !self.ui.chat_open;
        self.set_chat_open(open, window, cx);
    }

    fn set_chat_open(&mut self, open: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.ui.chat_open = open;
        if open {
            let panel = self.chat_panel(window, cx);
            panel.update(cx, |p, cx| p.focus_input(window, cx));
        } else {
            window.focus(&self.focus, cx);
        }
        self.persist(cx);
        cx.notify();
    }

    /// Open the panel with `prompt` typed and `files` attached, for the
    /// user to send (a PLS-SEM model's Explain results).
    pub(super) fn chat_prefill(
        &mut self,
        prompt: &str,
        files: &[std::path::PathBuf],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.set_chat_open(true, window, cx);
        let panel = self.chat_panel(window, cx);
        panel.update(cx, |p, cx| p.prefill(prompt, files.to_vec(), window, cx));
    }

    /// The docked panel and its resize handle, when open.
    pub(super) fn render_chat_dock(&self, cx: &mut Context<Self>) -> Option<Div> {
        if !self.ui.chat_open {
            return None;
        }
        let panel = self.chat.as_ref()?.panel.clone();
        let handle = div()
            .id("chat-handle")
            .absolute()
            .top_0()
            .bottom_0()
            .left(-HANDLE / 2.)
            .w(HANDLE)
            .cursor(CursorStyle::ResizeLeftRight)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|ws, ev: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    let drag = Drag::Chat {
                        origin_x: ev.position.x,
                        origin_w: ws.ui.chat_width,
                    };
                    ws.start_drag(drag, cx);
                }),
            );
        Some(
            div()
                .relative()
                .flex_none()
                .h_full()
                .w(px(clamp_width(self.ui.chat_width)))
                // Redrawn only when the panel changes, not with every edit
                // in a document.
                .child(panel.cached(div().size_full().style().clone()))
                .child(handle),
        )
    }

    /// Width the file-drop layer leaves to the panel (which takes its own
    /// drops as attachments).
    pub(super) fn chat_drop_gap(&self) -> Option<Pixels> {
        (self.ui.chat_open && self.chat.is_some()).then(|| px(clamp_width(self.ui.chat_width)))
    }
}

#[cfg(test)]
mod tests {
    use kantor_core::session::UiState;

    use super::*;

    #[std::prelude::v1::test]
    fn width_stays_in_bounds() {
        assert_eq!(clamp_width(10.), MIN_WIDTH);
        assert_eq!(clamp_width(5000.), MAX_WIDTH);
        assert_eq!(clamp_width(UiState::default().chat_width), 380.);
    }
}
