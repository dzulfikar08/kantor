#!/usr/bin/env bash
# Re-take all six screenshots (Sheets, Docs, Slides; dark and light) on Hyprland, then rebuild
# the site images and the Open Graph card. The README uses the same captures (docs/images/).
# Kantor runs on a private headless output, so nothing appears on your screen.
# Needs: a built Kantor (KANTOR_BIN), Omarchy themes in /usr/share/omarchy/themes, grim, jq,
# ImageMagick, rsvg-convert, and python with openpyxl, python-docx, python-pptx and pillow (PY).
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
PY=${PY:-python3}
samples=${SAMPLES:-$HOME/.cache/kantor-site-samples}
"$PY" -I "$here/make_samples.py" "$samples"
mcp="python3 -I $here/mcp_call.py"
themes=/usr/share/omarchy/themes

out=KSITE
hyprctl output create headless "$out" >/dev/null
trap 'hyprctl output remove "$out" >/dev/null' EXIT
# 1600x1000 at 1.25: a 1240x780 window is captured at 1550x975. Placed far from eDP-1.
hyprctl eval "hl.monitor({ output = '$out', mode = '1600x1000@60', position = '0x4000', scale = 1.25 })" >/dev/null
sleep 1
WS=$(hyprctl monitors -j | jq -r ".[] | select(.name==\"$out\") | .activeWorkspace.id")
export WS

for pair in dark:catppuccin light:catppuccin-latte; do
  m=${pair%%:*}; t=$themes/${pair#*:}/colors.toml
  POST="$mcp sheets_select '{\"range\":\"B5:E10\"}' >/dev/null" WAIT=8 "$here/capture.sh" "$t" "sheets-$m" "$here/shots" "$samples/sales.xlsx"
  POST=: WAIT=8 "$here/capture.sh" "$t" "docs-$m" "$here/shots" "$samples/report.docx"
  POST="$mcp slides_go_to '{\"slide\":3}' >/dev/null" WAIT=8 "$here/capture.sh" "$t" "slides-$m" "$here/shots" "$samples/review.pptx"
done
"$here/images.sh"
"$here/og.sh"
