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
        NewDocument,
        NewPresentation,
        OpenFile,
        OpenFromCloud,
        CloudAccounts,
        SyncNow,
        Print,
        CloseTab,
        ClosePane,
        ToggleSidebar,
        ToggleInspector,
        SplitRight,
        SplitDown,
        NextTab,
        PrevTab,
        GoHome,
    ]
);

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new(kantor_ui::keys::primary("ctrl-q"), Quit, None),
        KeyBinding::new("super-q", Quit, None),
        KeyBinding::new(
            kantor_ui::keys::primary("ctrl-k"),
            TogglePalette,
            Some(WORKSPACE),
        ),
        KeyBinding::new(
            kantor_ui::keys::primary("ctrl-shift-p"),
            TogglePalette,
            Some(WORKSPACE),
        ),
        KeyBinding::new(
            kantor_ui::keys::primary("ctrl-n"),
            NewSpreadsheet,
            Some(WORKSPACE_KEYS),
        ),
        KeyBinding::new(
            kantor_ui::keys::primary("ctrl-shift-n"),
            NewDocument,
            Some(WORKSPACE_KEYS),
        ),
        // Ctrl+M stays free for a new slide inside Slides.
        KeyBinding::new(
            kantor_ui::keys::primary("ctrl-alt-n"),
            NewPresentation,
            Some(WORKSPACE_KEYS),
        ),
        KeyBinding::new(
            kantor_ui::keys::primary("ctrl-o"),
            OpenFile,
            Some(WORKSPACE_KEYS),
        ),
        KeyBinding::new(
            kantor_ui::keys::primary("ctrl-p"),
            Print,
            Some(WORKSPACE_KEYS),
        ),
        KeyBinding::new(
            kantor_ui::keys::primary("ctrl-w"),
            ClosePane,
            Some(WORKSPACE_KEYS),
        ),
        KeyBinding::new(
            kantor_ui::keys::primary("ctrl-shift-w"),
            CloseTab,
            Some(WORKSPACE_KEYS),
        ),
        // Ctrl+B / Ctrl+I stay free for bold / italic.
        KeyBinding::new(
            kantor_ui::keys::primary("ctrl-shift-e"),
            ToggleSidebar,
            Some(WORKSPACE_KEYS),
        ),
        KeyBinding::new(
            kantor_ui::keys::primary("ctrl-shift-i"),
            ToggleInspector,
            Some(WORKSPACE_KEYS),
        ),
        KeyBinding::new(
            kantor_ui::keys::primary("ctrl-\\"),
            SplitRight,
            Some(WORKSPACE_KEYS),
        ),
        KeyBinding::new(
            kantor_ui::keys::primary("ctrl-alt-\\"),
            SplitDown,
            Some(WORKSPACE_KEYS),
        ),
        KeyBinding::new(
            kantor_ui::keys::primary("ctrl-tab"),
            NextTab,
            Some(WORKSPACE_KEYS),
        ),
        KeyBinding::new(
            kantor_ui::keys::primary("ctrl-pagedown"),
            NextTab,
            Some(WORKSPACE_KEYS),
        ),
        KeyBinding::new(
            kantor_ui::keys::primary("ctrl-shift-tab"),
            PrevTab,
            Some(WORKSPACE_KEYS),
        ),
        KeyBinding::new(
            kantor_ui::keys::primary("ctrl-pageup"),
            PrevTab,
            Some(WORKSPACE_KEYS),
        ),
        KeyBinding::new(
            kantor_ui::keys::primary("ctrl-shift-h"),
            GoHome,
            Some(WORKSPACE_KEYS),
        ),
        KeyBinding::new("escape", Dismiss, Some(PALETTE)),
        KeyBinding::new("up", PaletteUp, Some(PALETTE_INPUT)),
        KeyBinding::new("down", PaletteDown, Some(PALETTE_INPUT)),
        KeyBinding::new(kantor_ui::keys::primary("ctrl-p"), PaletteUp, Some(PALETTE)),
        KeyBinding::new(
            kantor_ui::keys::primary("ctrl-n"),
            PaletteDown,
            Some(PALETTE),
        ),
    ]);
}
