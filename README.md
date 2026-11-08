<p align="center"><img src="assets/icons/kantor.svg" width="128" alt="Kantor"></p>

# Kantor

A small, fast, native office suite for Linux, written in Rust on [GPUI](https://github.com/zed-industries/zed/tree/main/crates/gpui) (the UI framework behind Zed). Three apps in one binary: **Sheets**, **Docs** and **Slides**. First target: [Omarchy](https://omarchy.org) on Apple Silicon (aarch64 Linux).

> **Status: early.** Sheets can open, edit and save `.xlsx` (formulas via [IronCalc](https://github.com/ironcalc/IronCalc)). Docs and Slides are not started. See [plan.md](plan.md).

## Install (prebuilt, no compiling)

Every push to `main` publishes a rolling **nightly** build for aarch64 and x86_64 Linux.

```sh
curl -fsSL https://raw.githubusercontent.com/dzulfikar08/kantor/main/scripts/install.sh | sh
kantor budget.xlsx
```

This installs `~/.local/bin/kantor`, a **Kantor** launcher (the suite's Home screen) and a **Kantor Sheets** launcher. Pin a tagged release with `curl -fsSL …/install.sh | KANTOR_VERSION=v0.1.0 sh`. On Arch, `kantor-bin` from the AUR installs the latest release. Changes per release: [CHANGELOG.md](CHANGELOG.md).

## Using Sheets

`kantor` opens the Home screen (Docs · Sheets · Slides); `kantor file.xlsx` or `kantor --sheets` goes straight to Sheets.

| Action | Keys |
|---|---|
| Move | arrows, `Tab`/`Shift+Tab`, `Home`/`End`, `PgUp`/`PgDn` |
| Edit a cell | just type (replaces), or `Enter`/`F2`/double-click (keeps content) |
| Commit / cancel | `Enter` (moves down) / `Esc`; clicking another cell also commits |
| Clear a cell | `Delete` |
| Open / Save / Save as | `Ctrl`/`Super` + `O` / `S` / `Shift+S` |
| Command palette | `Ctrl K` (or `Ctrl Shift P`) |
| New spreadsheet / Open | `Ctrl N` / `Ctrl O` |
| Close pane / close tab | `Ctrl W` / `Ctrl Shift W` |
| Split right / down | `Ctrl \` / `Ctrl Alt \` (drag the divider to resize) |
| Sidebar / inspector | `Ctrl Shift E` / `Ctrl Shift I` (drag the edge to resize) |
| Theme / UI font | `Ctrl K` → *Choose Theme…* / *Choose UI Font…* |
| Next / previous tab | `Ctrl Tab` / `Ctrl Shift Tab` |
| Quit | `Ctrl`/`Super` + `Q` |

Edits happen in the cell itself (synced live with the formula bar). With an input method such as fcitx5, press `Enter` first, then compose.

By default the UI follows your Omarchy theme (live-reloading on `omarchy theme set`); *Choose Theme…* offers your Omarchy themes plus 900+ Ghostty themes. The UI font is SF Pro when installed (never bundled), else Inter / Adwaita Sans. Open tabs, splits and panel sizes are restored on the next launch. Settings: `~/.config/kantor/settings.toml`; session: `~/.local/state/kantor/workspace.json`.

## AI agents (MCP)

`kantor mcp` is an [MCP](https://modelcontextprotocol.io) server: Claude Code, Claude Desktop,
Cursor and other agents can read and edit the documents open in the **running** Kantor while you
watch. Every change is one undo step (`Ctrl Z` takes it back), the changed cells, paragraphs or
slide are selected, and the status bar says what the agent did. Nothing is saved until the agent
(or you) saves.

```sh
claude mcp add kantor -- kantor mcp      # Claude Code (palette: "Copy MCP Setup for Claude Code")
kantor mcp --print-config                # JSON for Claude Desktop, Cursor and others
```

```json
{ "mcpServers": { "kantor": { "command": "/home/you/.local/bin/kantor", "args": ["mcp"] } } }
```

If Kantor isn't running, the first tool call starts it (`kantor mcp --no-launch` returns an error
instead). Tools: `list_documents`, `open_document`, `new_document`, `save_document`,
`activate_document`, `get_selection`; Sheets `sheets_read_range`, `sheets_write_range`,
`sheets_select`, `sheets_format`, insert/delete rows and columns; Docs `docs_get_paragraphs`,
`docs_insert_paragraphs`, `docs_replace_paragraph`, `docs_delete_paragraphs`, `docs_find_replace`;
Slides `slides_list`, `slides_add_slide`, `slides_set_shape_text`, `slides_delete_slide`,
`slides_set_notes`, `slides_go_to`. The app listens only on a private Unix socket
(`$XDG_RUNTIME_DIR/kantor/`, on macOS `~/Library/Application Support/Kantor/run/`), readable by
you alone.
## AI features (optional) and privacy

AI is **off** until you turn it on in `Ctrl K` → *AI Settings* and choose a provider: Anthropic, OpenAI, OpenRouter, Groq, DeepSeek, Google Gemini, any OpenAI-compatible server, or **Ollama** running on your own computer (nothing leaves it). Kantor has no AI service of its own: your data goes **only to the provider you chose**, and only when you run an AI command. Each command sends only what it needs:

- *Ask AI for a Formula…* (Sheets): the column headers, up to 20 sample rows near the selection and the active cell's address — never the whole sheet;
- *Explain This Formula* (Sheets): the formula and the column headers;
- *Rewrite / Translate Selection…* (Docs): the selected paragraphs' text.

API keys are stored in the system keychain (Secret Service on Linux, Keychain on macOS); if none is available Kantor says so and keeps the key in `~/.config/kantor/ai-keys.toml`, readable only by you. Keys are never written to `settings.toml` or logs. What the provider does with your requests is governed by its own terms.

## Building

All builds run on **GitHub Actions** (`ubuntu-24.04-arm` and `ubuntu-24.04`); contributors don't need a powerful machine. If you do want to build locally:

```sh
./scripts/ci-deps.sh            # Ubuntu packages; on Arch: wayland libxkbcommon vulkan-icd-loader fontconfig
cargo run -p kantor -- file.xlsx
cargo test --workspace
```

Expect `target/` to reach 10–20 GB and the first build to need plenty of RAM.

## Layout

```
crates/
  kantor         binary: CLI, suite shell (tabs, Home, command palette, status bar), `kantor mcp`
  kantor_ui      design system: metrics, colour tokens from Omarchy, cached views
  kantor_core    shared non-GPUI pieces (fuzzy search, find, settings, session)
  kantor_ai      AI providers, streaming HTTP clients, prompts, keychain (no GPUI)
  kantor_theme   Omarchy palette parser (no GPUI)
  sheets_engine  IronCalc wrapper: open/save xlsx, cells, formulas (no GPUI)
  sheets_ui      grid, formula bar, sheet tabs
vendor/
  gpui-pre-wgpu  GPUI's wgpu renderer with memory patches for 8 GB Apple Silicon
```

The shell's structure and styling follow [Voltrus Tools](https://github.com/VoltrusID/voltrus-explorers) (see `docs/reference-voltrus-tools.md`).

More crates (`docs_*`, `slides_*`, `kantor_ooxml`, `kantor_pdf`, …) arrive with their milestones.

## License

[GNU Affero General Public License v3.0 or later](LICENSE) (AGPL-3.0-or-later). Modified versions, including ones offered over a network, must publish their source under the same licence.
