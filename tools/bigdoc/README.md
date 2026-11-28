# Long-document timing for Kantor Docs

- `gen.py` writes reproducible 1,000+ page `.docx` files into
  `~/.cache/bigdoc` (contract compilation, thesis with pictures and
  footnotes, reviewed contract with tracked changes and comments). Only the
  standard library (plus ImageMagick for JPEG photos, if installed).
- `kantor docs bench FILE.docx [--limit N] [--verify]` (hidden command)
  times open, layout, typing on the first/middle/last page, Enter and
  Backspace, Find All and Save without a window; `--verify` checks every
  layout after an edit against a fresh full layout.
- In the app: `KANTOR_PERF=1` logs `[docs] open …` (read, first layout,
  first page shown, page count final), `[docs] input KEY→paint N ms` for
  every key press and wheel event, and `[perf] docs …` layout phases.
  `KANTOR_PERF_DOCS_SCROLL=N` adds a scroll benchmark once the page count
  is final (N frames of wheel-sized steps, N frames of page steps, jumps
  to the end and the middle).
- `rig/` drives the app headless on Hyprland: `rig/xsetup.sh` once, then
  `rig/xrun.sh BIN FILE rig/type.steps OUT.log` (typing on page 1, after
  Find on the middle page, after Ctrl+End at the end, Page Down, Enter,
  Save) or `rig/scroll.steps` with `KANTOR_PERF_DOCS_SCROLL=150` as the
  last argument. `perf_stats.py OUT.log` sums a log up per step.
