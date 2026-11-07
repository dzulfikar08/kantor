//! Every workspace action and key binding, in one place (Voltrus Tools pattern).
//!
//! Context guards keep shortcuts from fighting text fields and overlays.

use gpui_kit::*;

pub const WORKSPACE: &str = "Workspace";
pub const PALETTE: &str = "Palette";
/// The palette's text field: arrows move the selection instead of the caret.
const PALETTE_INPUT: &str = "Palette > Input";
const WORKSPACE_KEYS: &str = "Workspace && !Palette";

actions!(
    kantor,
    [
        Quit,
        TogglePalette,
        Dismiss,
        PaletteUp,
        PaletteDown,
        NewSpreadsheet,
        OpenFile,
        CloseTab,
        NextTab,
        PrevTab,
        GoHome,
    ]
);

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("ctrl-q", Quit, None),
        KeyBinding::new("super-q", Quit, None),
        KeyBinding::new("ctrl-k", TogglePalette, Some(WORKSPACE)),
        KeyBinding::new("ctrl-shift-p", TogglePalette, Some(WORKSPACE)),
        KeyBinding::new("ctrl-n", NewSpreadsheet, Some(WORKSPACE_KEYS)),
        KeyBinding::new("ctrl-o", OpenFile, Some(WORKSPACE_KEYS)),
        KeyBinding::new("ctrl-w", CloseTab, Some(WORKSPACE_KEYS)),
        KeyBinding::new("ctrl-tab", NextTab, Some(WORKSPACE_KEYS)),
        KeyBinding::new("ctrl-pagedown", NextTab, Some(WORKSPACE_KEYS)),
        KeyBinding::new("ctrl-shift-tab", PrevTab, Some(WORKSPACE_KEYS)),
        KeyBinding::new("ctrl-pageup", PrevTab, Some(WORKSPACE_KEYS)),
        KeyBinding::new("ctrl-shift-h", GoHome, Some(WORKSPACE_KEYS)),
        KeyBinding::new("escape", Dismiss, Some(PALETTE)),
        KeyBinding::new("up", PaletteUp, Some(PALETTE_INPUT)),
        KeyBinding::new("down", PaletteDown, Some(PALETTE_INPUT)),
        KeyBinding::new("ctrl-p", PaletteUp, Some(PALETTE)),
        KeyBinding::new("ctrl-n", PaletteDown, Some(PALETTE)),
    ]);
}
