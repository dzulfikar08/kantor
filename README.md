# Kantor

A small, fast, native office suite for Linux, written in Rust on [GPUI](https://github.com/zed-industries/zed/tree/main/crates/gpui) (the UI framework behind Zed). Three apps in one binary: **Sheets**, **Docs** and **Slides**. First target: [Omarchy](https://omarchy.org) on Apple Silicon (aarch64 Linux).

> **Status: early.** Sheets can open, edit and save `.xlsx` (formulas via [IronCalc](https://github.com/ironcalc/IronCalc)). Docs and Slides are not started. See [plan.md](plan.md).

## Install (prebuilt, no compiling)

Every push to `main` publishes a rolling **nightly** build for aarch64 and x86_64 Linux.

```sh
curl -fsSL https://raw.githubusercontent.com/dzulfikar08/kantor/main/scripts/install.sh | sh
kantor budget.xlsx
```

This installs `~/.local/bin/kantor` and a *Kantor Sheets* launcher entry. Pin a release with `KANTOR_VERSION=v0.1.0`.

## Using Sheets

| Action | Keys |
|---|---|
| Open / Save / Save as | `Ctrl`/`Super` + `O` / `S` / `Shift+S` |
| Edit a cell | select it, type in the formula bar, `Enter` (double-click jumps to the bar) |
| Quit | `Ctrl`/`Super` + `Q` |

The UI follows your Omarchy theme and live-reloads on `omarchy theme set`.

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
  kantor         binary: CLI, windows, Omarchy theme -> GPUI theme
  kantor_theme   Omarchy palette parser (no GPUI)
  sheets_engine  IronCalc wrapper: open/save xlsx, cells, formulas (no GPUI)
  sheets_ui      grid, formula bar, sheet tabs
```

More crates (`docs_*`, `slides_*`, `kantor_ooxml`, `kantor_pdf`, …) arrive with their milestones.

## License

Dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option.
