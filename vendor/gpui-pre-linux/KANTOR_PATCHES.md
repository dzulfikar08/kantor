# Kantor patches to gpui-pre-linux 0.3.11 (Apache-2.0)

Vendored from crates.io `gpui-pre-linux` 0.3.11 and wired in with `[patch.crates-io]`
in the workspace `Cargo.toml`. Changes (search for `KANTOR PATCH`):

1. X11: the first mouse-wheel notch after the pointer enters a window is no longer lost.
   GPUI scrolls from XInput2 smooth-scroll valuators and computes each delta against the
   previous valuator value. That value is forgotten when the pointer leaves the window or
   focus changes (the valuator is global, so an old value would make a huge jump), so the
   next smooth-scroll event only set a new baseline and produced no scroll. The X server
   follows each smooth-scroll event with an emulated wheel button press (flagged
   `POINTER_EMULATED`), which GPUI always skipped. Now `ScrollAxisState.use_emulated_buttons`
   records that the last smooth-scroll event on that axis had no baseline, and the emulated
   press that follows it scrolls `SCROLL_LINES` instead. Later notches still come from the
   valuators (exact deltas, touchpads unaffected).

   Upstream (zed-industries/zed `crates/gpui_linux/src/linux/x11/client.rs`, unchanged on main
   as of 2026-11-18) can take the same patch.

2. Wayland: a window opened with `WindowParams::display_id` goes fullscreen on that output.
   `xdg_toplevel.set_fullscreen` was always sent with no output, so the compositor chose one
   (usually the focused screen) and a slide show opened for the projector covered the laptop
   screen instead. `WaylandWindowState.fullscreen_output` keeps the requested output and
   `toggle_fullscreen` passes it. Windows without a `display_id` are unchanged.

3. The window appearance starts as dark instead of light (`LinuxCommon::new`). It changes
   as soon as the XDG desktop portal reports a colour scheme (`prefer-light` and
   `no-preference` give light, `prefer-dark` dark), so only a desktop without the portal's
   Settings interface keeps it. Kantor's "Follow system" theme uses it, and a desktop that
   can't say what it prefers gets Kantor's usual dark look.

Drop this vendor copy once upstream ships the fixes (patch 3 is Kantor's choice, not a fix).
