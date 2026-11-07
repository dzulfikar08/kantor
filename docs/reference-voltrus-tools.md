# Reference: Voltrus Tools GPUI layout

Source: `VoltrusID/voltrus-explorers`, `apps/voltrus-tools` (v1.0.3, read 2026-11-07).
Paths below are relative to `apps/voltrus-tools/crates/`. It targets macOS first
(Mac App Store and Developer ID DMG builds). Linux builds work, but the shell has
no Linux-specific chrome code.

Stack: raw `gpui = "0.2.2"` patched to `VoltrusID/gpui@006435b` (a fork of
`muxy-app/gpui`). It does **not** use gpui-component. All widgets live in its own
`vt-ui` crate. Most of the structure is credited to muxy (github.com/muxy-app/muxy).

## 0. Crate layering (`docs/architecture.md`, root `Cargo.toml`)

| Crate | Role | GPUI? |
| --- | --- | --- |
| `vt-protocol` | Commands and events, plain serde data | no |
| `vt-engine` | One tokio task per session on its own runtime thread | no |
| `vt-app-core` | Saved workspace, tabs, split tree (`layout.rs`), fuzzy search (`search.rs`), atomic store | no |
| `vt-ui` | Theme tokens, metrics, components, text input, charts. No app state | yes |
| `vt-app` | The binary: one `AppModel` entity, with views as plain functions over it | yes |

The allowed edges are enforced as data in
`[workspace.metadata.architecture.allowed-dependencies]`. Workspace lints:
`unsafe_code = deny`, `clippy::unwrap_used = deny`, `todo/dbg/print_stdout = deny`.

## 1. Window shell

### Window options (`vt-app/src/main.rs`)
```rust
let bounds = Bounds::centered(None, size(px(1440.0), px(900.0)), cx);
let options = WindowOptions {
    window_bounds: Some(WindowBounds::Windowed(bounds)),
    titlebar: Some(TitlebarOptions {
        title: Some("Voltrus Tools".into()),
        appears_transparent: true,                       // macOS: content under titlebar
        traffic_light_position: Some(point(px(14.0), px(13.0))),
    }),
    window_min_size: Some(size(px(960.0), px(600.0))),
    app_id: Some("id.voltrus.tools".into()),            // Wayland app_id / X11 class
    ..WindowOptions::default()
};
```
- **Decorations:** `window_decorations` is left unset. In gpui `Window::new` this
  becomes `request_decorations(window_decorations.unwrap_or(WindowDecorations::Server))`,
  so the app always asks for server-side decorations. On macOS the traffic lights sit
  over the app's own 38 px top row. On Wayland and Hyprland the compositor draws no
  titlebar, so you get just the app's top row. There is **no** `start_window_move`,
  `WindowControlArea` or client-side close/min/max button anywhere in the repo
  (grep confirmed). Window moves and closes are left to the WM.
- **Platform branches** are all `cfg!(target_os = "macos")`:
  - Left padding of 78 px for the traffic lights (`TRAFFIC_LIGHT_WIDTH`). Elsewhere it is `SPACE_4` = 8 px
    (`views/tabs.rs`, `views/sidebar.rs`, `views/welcome.rs`).
  - The primary modifier is `cmd` on macOS and `ctrl` elsewhere (`actions.rs`, `vt-ui/src/text_input.rs`).
  - Native menus come from `cx.set_menus(actions::menus())`. On Linux they are effectively unused, so
    every command also needs a key binding or a palette entry.

### Root layout (`vt-app/src/views/mod.rs`, `impl Render for AppModel`)
```
root (v-flex, bg, SANS, FONT_BODY, key_context "Workspace", track_focus)
├─ h-flex flex_1
│  ├─ sidebar         CachedView, w = ui.sidebar_width (248), border_r
│  └─ content (v-flex flex_1)
│     ├─ tabs strip   CachedView, h = TITLE_BAR_HEIGHT (38): the tab bar IS the titlebar
│     └─ h-flex flex_1
│        ├─ splits::render (active tab's split tree) | welcome::render
│        └─ inspector CachedView, w = ui.inspector_width (300)
├─ status bar         CachedView, h = STATUS_BAR_HEIGHT (26), border_t
├─ overlay            palette | editor sheet | "+" menu (absolute, inset_0, occlude)
└─ toast              absolute bottom 40 / right 16, elevated, Popover shadow
```
- The sidebar has its own 38 px header row holding the "hide sidebar" IconButton, so it lines
  up with the tab row. When the sidebar is hidden, the tab strip shows a "show sidebar"
  button instead (`views/tabs.rs`).
- **Cached sub-views** (`views/cached.rs`) are the main performance pattern. Each chrome region is an
  `Entity<CachedView>` that observes the model and is drawn with `AnyView::cached(style)`.
  An animating pane then redraws only itself and the thin shell; the rest replays its last frame.
```rust
pub struct CachedView { model: WeakEntity<AppModel>, render: RenderFn, _model_changes: Subscription }
impl CachedView {
    pub fn new(render: impl Fn(&mut AppModel, &mut Window, &mut Context<AppModel>) -> AnyElement + 'static,
               cx: &mut Context<AppModel>) -> Entity<Self> {
        let model = cx.entity();
        cx.new(|cx| Self { model: model.downgrade(), render: Box::new(render),
                           _model_changes: cx.observe(&model, |_, _, cx| cx.notify()) })
    }
}
// usage in root:
AnyView::from(chrome.tabs.clone())
    .cached(div().flex_none().w_full().h(TITLE_BAR_HEIGHT).style().clone())
```
  `model::Chrome { sidebar, tabs, inspector, status_bar, panes: HashMap<PaneId, Entity<CachedView>> }`
  is built in `model/mod.rs` (around line 246). Panes are created lazily in `sync_pane_views`.

### Tabs (`views/tabs.rs`)
- Each tab is 28 px tall with `RADIUS_MD`, `FONT_SMALL` at MEDIUM weight, and a fixed title width of 150 px with
  `truncate()`. The comment explains why: "text only truncates inside a known width".
- The selected tab gets `bg(theme.bg)`, `border_1` and `border_strong` on a `chrome` strip (an inset "pill"
  look). Other tabs use `fg_muted` with hover `theme.hover`.
- A status dot sits on the left, a `+N` badge counts extra split panes, and the close `IconButton` (18 px) is
  shown on group-hover or when the tab is selected (`.invisible().group_hover(group, |s| s.visible())`).
- The strip uses `overflow_x_scroll()`, then a "+" button that opens the command palette. Split right,
  split down and inspector-toggle IconButtons are pinned on the right.

### Splits (`vt-app-core/src/layout.rs`, `views/splits.rs`)
- A binary tree, `enum Layout { Leaf(PaneId), Split { axis, ratio, first, second } }`, with ratio clamped
  to `0.15..=0.85`. Paths are `Vec<Branch>` (`First`/`Second`). `split`, `without` and `set_ratio`
  are pure functions and serde-serialisable, so layouts persist in the workspace file.
- Rendering: the first child gets `w(relative(ratio))`, then a 1 px `theme.border` divider with an absolute
  7 px hit area (`left(px(-3.0))`) and resize cursor, then the second child `flex_1`. A
  `canvas(...)` records each split's bounds into `split_bounds: Rc<RefCell<HashMap>>` so the
  drag code can convert pixel deltas to ratios. Drag state lives in `AppModel.drag`, and the root
  `on_mouse_move` and `on_mouse_up` drive it.
- The focused pane in a split tab gets an accent edge (`views/pane.rs`).
- Panes are responsive: widths measured at layout go into `pane_widths`. Thresholds are `COMPACT = 720`
  (segmented switcher goes icon-only) and `NARROW = 520` (status moves under the name, Connect becomes an icon).

### Side panels
`views/mod.rs::resize_handle(id, on_down)` returns a 6 px absolute strip with
`CursorStyle::ResizeLeftRight`, which the caller positions on the panel edge. Widths are
persisted in `UiState { sidebar_open, sidebar_width: 248, inspector_open, inspector_width: 300,
appearance, dark_theme, light_theme }` (`vt-app-core/src/workspace.rs`).

### Status bar (`views/status_bar.rs`)
It is 26 px tall with `px(SPACE_6)`, `gap(SPACE_6)`, `bg(chrome)`, `border_t_1`, and `FONT_CAPTION` in `fg_muted`. Left side: a live
dot with "N live · M connecting", "N saved", and a frames counter in `MONO`. Then `flex_1`. Right side: licence item,
a clickable theme name that opens the theme palette, and the version.

### Pane header and view switcher (`views/pane.rs`)
Header items: monogram, then name and endpoint, then a `segmented()` control (Points/Trend/Traffic/Log/Device,
each with an icon), then a primary/secondary session `Button`. This works as a per-document toolbar.

## 2. Design tokens (`vt-ui/src/theme/metrics.rs`, `tokens.rs`)

**Type** (px): caption 10.5, small 11.5, **body 12.5**, emphasis 13, title 15, display 22, value 26.
Fonts: `SANS = ".SystemUIFont"` (San Francisco on macOS; on Linux gpui resolves its own
fallback), `MONO = "JetBrains Mono"`, which is bundled and registered in `vt_ui::init` via
`cx.text_system().add_fonts`.

**Spacing**: `SPACE_1..10` = 2, 4, 6, 8, 10, 12, 16, 20, 24, 32.
**Icons**: 12 / 14 / 16. **Controls**: SM 22, MD 26, LG 32.
**Radii**: 4 / 6 / 8 / 12. **Shell**: titlebar 38, status bar 26, table row 28, palette row 36,
palette footer 34, sidebar connection row 44, sidebar section header 24.
**Borders**: always `border_1()` (1 px) with `theme.border`, or `border_strong` for selected and floating
items. Split dividers are 1 px.

**Colours** come from one Ghostty-format scheme (492 bundled; house themes `Voltrus` and
`Voltrus Light`). `Theme::from_scheme` derives all of them, with `on_bg(a)` = fg blended over bg at alpha a:

| token | dark | light |
| --- | --- | --- |
| `chrome` (sidebar/tabs/status/inspector) | on_bg(0.03) | on_bg(0.025) |
| `surface` (cards, inputs, table headers) | on_bg(0.05) | on_bg(0.035) |
| `elevated` (menus, dialogs) | on_bg(0.07) | #ffffff |
| `fg_muted` / `fg_dim` | fg @ 0.68 / 0.42 | same |
| `border` / `border_strong` | on_bg(0.10) / (0.18) | on_bg(0.11) / (0.20) |
| `hover` | fg @ 0.06 | same |
| `selected` | accent @ 0.16 | accent @ 0.12 |
| `accent` | palette[4] (fallback #22c55e) | same |
| `accent_soft` | accent @ 0.12 | same |
| `accent_fg` | black if luminance(accent) > 0.6, else white | same |
| success / warning / danger / info | palette 2 / 3 / 1 / 6 | same |

Shadows (`Theme::shadow(Elevation)`), with strength 1.0 dark and 0.55 light:
Raised (0,1) blur 2 at α .18; Popover (0,2,6,.22)+(0,8,24,.18); Modal (0,4,12,.25)+(0,16,48,.3).
The modal scrim is black at α .45 (dark) or .2 (light). Rule: "views never use raw colours or sizes".
The theme lives in a `Global` (`ActiveTheme`), and `set_theme` calls `cx.refresh_windows()`. Palette
theme entries preview live and are restored on Esc.

## 3. Components: vt-ui and the gpui-component 0.9 equivalents

`vt-ui` is about 2.3 kLOC on raw gpui: stateless `#[derive(IntoElement)]` + `RenderOnce`
builders that read `theme(cx)`. It has its own `TextInput` entity (1 kLOC, key context
`TextInput`) and its own charts with `History` ring buffers. Lists use `gpui::uniform_list`.

| vt-ui (`components.rs`) | Notes | gpui-component 0.9 we already have |
| --- | --- | --- |
| `Button` (Primary/Secondary/Ghost/Danger, `.compact()`, `.icon()`) | h 26 or 22, px 12 or 8, RADIUS_MD, `tab_index(0)`, focus ring = border colour | `button::Button` + `ButtonVariants` (`.primary() .ghost() .danger()`), `.small()` |
| `IconButton` (`.size() .active() .tooltip()`) | 22 square, `selected` bg when active | `Button::new(id).icon(..).ghost()` |
| `segmented(id, Vec<Segment>)` | pane view switcher | `button::ButtonGroup` or `tab::TabBar` (segmented variant) |
| `Tooltip::view` | | `tooltip::Tooltip` |
| `kbd("⌘K")` | caption, MONO, border, surface | `kbd::Kbd` |
| `section_label` | caption SEMIBOLD uppercase fg_dim | `label::Label` + styling |
| `status_dot`, `pill`, `monogram` | | `badge::Badge`, `tag::Tag`, `avatar` |
| `field(label, control)` | label above control | `form` module |
| `checkbox`, `empty_state`, `divider` | | `checkbox::Checkbox`, `empty`, `separator` |
| `text_input::TextInput` | own entity, cmd/ctrl bindings | `input::{InputState, Input}` |
| points table: `uniform_list` rows at `ROW_HEIGHT` 28 | hand-rolled | `table::DataTable<TableDelegate>` |
| tabs strip (hand-rolled) | | `tab::TabBar` / `Tab` |
| split tree (hand-rolled) | | `resizable::{h_resizable, v_resizable}` or `dock` |
| status bar (hand-rolled) | | `status_bar::StatusBar` |
| no titlebar component | | `title_bar::TitleBar` (34 px, CSD on Linux, `start_window_move`) |
| layouts use `div().flex().flex_col()` | | `h_flex()` / `v_flex()` |

In practice, for Kantor: keep gpui-component widgets, but copy vt-ui's **token discipline**
(one metrics module plus derived colour tokens) and its **shell structure**.

## 4. Keyboard, focus, palette, "many tools in one binary"

- **Actions**: one `actions!(voltrus_tools, [...])` list and one `key_bindings()` in `vt-app/src/actions.rs`,
  with a `menus()` function alongside it. Key contexts are string constants:
```rust
pub const WORKSPACE: &str = "Workspace";
pub const PALETTE: &str = "Palette";
pub const EDITOR: &str = "Editor";
const PANE_KEYS: &str = "Workspace && !Editor && !Palette";
const POINT_KEYS: &str = "Workspace && !Editor && !Palette && !TextInput";
let primary = if cfg!(target_os = "macos") { "cmd" } else { "ctrl" };
KeyBinding::new(&key("k"), ToggleCommandPalette, None),
KeyBinding::new(&key("d"), SplitRight, Some(PANE_KEYS)),
KeyBinding::new("down", SelectNextPoint, Some(POINT_KEYS)),   // arrows yield to text fields
KeyBinding::new("up", PaletteUp, Some(PALETTE)),
```
- **Shortcuts**: palette `mod-k` and `mod-shift-p`; `mod-n` new; `mod-b` sidebar; `mod-i` inspector;
  `mod-d` / `mod-shift-d` split; `mod-w` close pane; `ctrl-tab`, `mod-shift-]` / `[` cycle tabs;
  `mod-1..5` switch the pane view; `mod-enter` toggles the session; `escape` → `Dismiss`.
- **Handlers**: every action is handled once on the root div via `.on_action(cx.listener(...))` and
  forwarded to an `AppModel` method. There is a single root `FocusHandle` (`model.focus`) and
  `window.focus(&self.focus)` after each overlay closes. The palette sets
  `.key_context(PALETTE)` on its panel so its arrow and enter bindings shadow the workspace ones.
- **Overlays**: `enum Overlay { None, Palette(_), Editor(_), NewMenu{..} }`. There is one at a time,
  rendered last as an absolute `inset_0().occlude()` layer. A scrim `on_mouse_down` closes it, and
  the panel calls `cx.stop_propagation()`.
- **Command palette** (`views/palette.rs`, `model/palette.rs`, `vt-app-core/src/search.rs`):
  600×470 panel, 96 px from the top, `RADIUS_XL`, `elevated`, Modal shadow. Header holds a search icon and
  TextInput. Body is a `uniform_list` of 36 px rows (icon, title, dim detail) with selection held in the model,
  `scroll_to_item(.., Center)`. Footer has kbd hints and a result count. `Mode::{Commands, Themes}`
  share the panel. The fuzzy scorer awards word-start +8, text-start +6, consecutive +5, gap −1.
- **Many tools in one window**: there is no per-app mode switch. Every protocol is a `Connection` in
  one `Workspace { connections, panes, tabs, active_tab, ui }`. A tab holds a split tree of panes, and
  each pane shows one connection with a `PaneView`. Per-protocol behaviour dispatches on the spec
  (`EditorKind::for_protocol`, `views/device.rs` hands off to the protocol module). For Kantor this maps to
  `Document { kind: Docs|Sheets|Slides }` per tab or pane. Shared chrome stays the same, and the
  toolbar or inspector dispatches on `kind`.
- **Persistence**: the whole `Workspace` (tabs, layout, widths, theme) is serde JSON written
  atomically. It is saved on quit and on `on_window_should_close`. A corrupt file loads the defaults
  and shows a toast instead of overwriting.

## 5. The VoltrusID/gpui fork and Linux memory

Fork commits on top of `gpui 0.2.2` (`gh api repos/VoltrusID/gpui/commits?sha=006435b`):

| commit | change | platform |
| --- | --- | --- |
| 64c8ce4 | Path MSAA texture uses **memoryless** storage (tile memory). This removes a window-sized 4× MSAA allocation that became resident once any path was drawn | macOS/Metal only |
| bd450ea | Release path intermediate textures when a frame draws no paths, and allocate them only when it does | Metal only |
| b28d03b | Instance-buffer pool sized to the current frame (next pow2, min 2 MiB, max 256 MiB). It shrinks again, where before it only grew | Metal only |
| e96f591 | Fix oversized-image ownership in the Metal atlas (leak) | Metal only |
| 6fc3226 | Frame pacing: keep presenting at refresh rate only while invalidating input arrives at ≥60/s (100 ms window, 1 s sustain). Before, every input caused 1 s of full-rate presents | cross-platform (`src/window.rs`) |
| f9f482e, 0f0fecb | Linux clipboard `text/uri-list` → `ExternalPaths`; X11 raw window handles | Linux |
| 5c1b811, 6c718f3, taffy bump | App Store and AppKit fixes | macOS |

**Relevance to our `wgpu error: Out of Memory` and device-lost on Asahi.** The fork does not
touch any wgpu or Vulkan code. gpui 0.2.2 used blade on Linux; we are on `gpui-pre-wgpu 0.3.11`. The same
*problems* do exist in our renderer, though. I checked the local source at
`scratchpad/src/wgpu/gpui-pre-wgpu-0.3.11/src/wgpu_renderer.rs`:
- `ensure_intermediate_textures(size)` runs **every frame, whether or not there are paths**
  (line ~1462). It allocates a window-sized `path_intermediate` texture **plus** a `path_msaa` texture with
  `path_sample_count = [4,2,1].find(supported)`, which means 4× on Asahi (`RenderingParameters::new`, ~2611).
  At 2560×1600 in BGRA8 that is about 16 MB + 66 MB of ordinary (not memoryless) RAM on unified memory,
  reallocated on every resize. No env var overrides the sample count.
- `grow_instance_data` doubles the instance buffer and **never shrinks** it (~2089).
- The present mode defaults to Fifo with `desired_maximum_frame_latency: 2`.

Options, in order of cost:
(a) App side: avoid needless redraws (the CachedView pattern, no continuous animation, no
`request_animation_frame` loops), keep one window, and avoid huge single-frame scenes (virtualise
lists and grids so the instance buffer stays small). Throttle resize-driven re-layout.
(b) Vendor `gpui-pre-wgpu` via `[patch.crates-io]` and port the fork's ideas: create the path
textures only when `scene.paths` is non-empty and drop them otherwise, and force `path_sample_count = 1`
(or read an env var). Optionally shrink the instance buffer when demand falls. This is the direct analogue of
bd450ea + 64c8ce4 + b28d03b.
(c) Port 6fc3226 (input-rate frame pacing) if `gpui-pre`'s `window.rs` still has the old
"present for 1 s after any input" logic. Check `last_input_timestamp` in gpui-pre 0.3.11 before doing this.

## 6. Recommendations for Kantor (ranked)

1. **Cached chrome views.** Wrap sidebar, tab strip, inspector, status bar and each document pane
   in `CachedView` + `AnyView::cached`, so typing in a Sheets cell does not rebuild the shell. It is
   cheap to copy (`views/cached.rs`, 48 lines).
2. **One metrics module + derived colour tokens.** Copy `metrics.rs` values (body 12.5, rows 28,
   controls 22/26/32, radii 4/6/8/12, spacing 2…32) and the `chrome/surface/elevated/border/hover/selected`
   recipe. Map them onto gpui-component's `ThemeColor` at startup instead of scattering `px()`.
3. **Shell geometry:** a 38 px top row where the tab strip doubles as the titlebar, a 248 px resizable left
   sidebar with a matching 38 px header, a 300 px toggleable right inspector, a 26 px status bar, and 1 px
   borders. On Hyprland, keep server decorations (the default) and draw no window buttons. That
   matches Voltrus on Linux, and Omarchy users tile anyway. If you want gpui-component's `TitleBar`, use it
   only for drag and double-click.
4. **Actions and key contexts as constants** in one `actions.rs`, with `mod = cmd|ctrl`, context guards like
   `"Workspace && !Palette && !TextInput"`, all handlers on the root, and one root focus handle
   restored after overlays.
5. **Command palette with mod-k / mod-shift-p** as the primary entry point on Linux, since native menus don't
   show. Reuse `search.rs`'s scorer (around 60 lines). Use gpui-component `command` if it fits.
6. **Persisted workspace model** (`Workspace`/`Tab`/`Layout` split tree, `UiState` widths) as plain
   serde in a non-GPUI crate. Fall back to defaults when the file is corrupt, and save on window close.
7. **Responsive panes:** measure width with `canvas` and switch the toolbar to icon-only below about 720 px. This
   matters on a 13" Air with splits.
8. **Renderer memory:** patch `gpui-pre-wgpu` to allocate path textures lazily with sample count 1
   (section 5b). This is the most likely real fix for the Asahi OOM, and the only fork idea that carries over.
