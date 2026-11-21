# Kantor patches to gpui-pre-macos 0.3.11 (Apache-2.0)

Vendored from crates.io `gpui-pre-macos` 0.3.11 and wired in with `[patch.crates-io]`
in the workspace `Cargo.toml`. Changes (search for `KANTOR PATCH`):

1. Feature `mas` (Mac App Store build, turned on by `kantor`'s `app-store` feature): no
   private Objective-C selectors in the binary. App Review rejects apps whose binary names
   non-public selectors (ITMS-90338), and GPUI uses three:
   - `_opaqueRectForWindowMoveWhenInTitlebar` (an override on GPUI's view: lets windows that
     drag themselves claim the whole titlebar). Kantor windows use AppKit's own titlebar
     dragging, so the store build simply doesn't add the override.
   - `_windowRestorationOptions` (an override on the window-state unarchiver that puts a
     restored window back on its original Space on macOS 15+). Without it a restored window
     opens on the current Space. Kantor restores its own session and doesn't use GPUI's
     native window state, so nothing changes for Kantor.
   - `_zoomFill:` (the Window ▸ Fill action). The store build calls the public `zoom:`.

   Without the feature the crate is unchanged, so the Developer ID build behaves exactly
   as before. `scripts/build-mas.sh check-private` checks a built binary for private
   symbols and selectors.

2. `set_next_open_panel(directory, message)`: the next `prompt_for_paths` panel starts in
   `directory` and shows `message` (`NSOpenPanel.directoryURL` / `message`). GPUI's
   `PathPromptOptions` has no field for either. The sandboxed store build uses it for
   "Grant File Access": when Kantor knows a file's path (last session's tabs, recent files,
   a linked data source or picture) but macOS hasn't given it access, the panel opens on
   that file's folder and says why. Not called by the Developer ID build.

Drop this vendor copy once upstream offers a switch for it (or stops using the selectors).
