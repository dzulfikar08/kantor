#!/usr/bin/env bash
# Re-take all six screenshots (Sheets, Docs, Slides; dark and light) on Hyprland, then rebuild
# the site images and the Open Graph card. The README uses the same captures (docs/images/).
# Kantor runs on a private headless output, so nothing appears on your screen.
# Needs: a built Kantor (KANTOR_BIN), Omarchy themes in /usr/share/omarchy/themes, grim, jq,
# Xwayland, xdotool, ImageMagick, rsvg-convert, and python with openpyxl, python-docx and
# python-pptx (PY).
# ONLY=docs-light (a tag, or a list of tags) retakes just those and skips images.sh and og.sh.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
PY=${PY:-python3}
samples=${SAMPLES:-$HOME/.cache/kantor-site-samples}
"$PY" -I "$here/make_samples.py" "$samples"
mcp="python3 -I $here/mcp_call.py"
themes=/usr/share/omarchy/themes
export XDISP=${XDISP:-:96}

out=KSITE
xpid=""
cleanup() {
  [ -n "$xpid" ] && kill "$xpid" 2>/dev/null || true
  hyprctl output remove "$out" >/dev/null
}
hyprctl output create headless "$out" >/dev/null
trap cleanup EXIT
# 1600x1000 at 1.25: a 1240x780 window is captured at 1550x975. Placed far from eDP-1.
hyprctl eval "hl.monitor({ output = '$out', mode = '1600x1000@60', position = '0x4000', scale = 1.25 })" >/dev/null
sleep 1
WS=$(hyprctl monitors -j | jq -r ".[] | select(.name==\"$out\") | .activeWorkspace.id")
export WS
# Docs needs key presses (to scroll the page), which go to a private X server on the same output.
hyprctl eval "hl.exec_cmd('Xwayland $XDISP -geometry 1550x975 -noreset', { workspace = '$WS silent' })" >/dev/null
for _ in $(seq 1 40); do DISPLAY=$XDISP xdotool getdisplaygeometry >/dev/null 2>&1 && break; sleep 0.25; done
xpid=$(pgrep -f "^Xwayland $XDISP " | head -1)

want() { [ -z "${ONLY:-}" ] || [[ " $ONLY " == *" $1 "* ]]; }
for pair in dark:catppuccin light:catppuccin-latte; do
  m=${pair%%:*}; t=$themes/${pair#*:}/colors.toml
  # Sheets: the year's totals selected (the status bar sums them).
  want "sheets-$m" && POST="$mcp sheets_select '{\"range\":\"F5:F12\"}' >/dev/null" WAIT=8 \
    "$here/capture.sh" "$t" "sheets-$m" "$here/shots" "$samples/sales.xlsx"
  # Docs: the lower half of page 1 (table, tracked changes, comment, footnotes).
  want "docs-$m" && WAIT=8 STEPS="${DOCS_STEPS:-key ctrl+End
sleep 1}" "$here/xcapture.sh" "$t" "docs-$m" "$here/shots" "$samples/report.docx"
  want "slides-$m" && POST="$mcp slides_go_to '{\"slide\":3}' >/dev/null" WAIT=8 \
    "$here/capture.sh" "$t" "slides-$m" "$here/shots" "$samples/review.pptx"
done
[ -n "${ONLY:-}" ] && exit 0
"$here/images.sh"
"$here/og.sh"
