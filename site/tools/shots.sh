#!/usr/bin/env bash
# Re-take all six site screenshots (Sheets, Docs, Slides; dark and light) on Hyprland,
# then rebuild the site images and the Open Graph card.
# Needs: a built Kantor (KANTOR_BIN), Omarchy themes in /usr/share/omarchy/themes,
# python3 with openpyxl, python-docx and python-pptx (PY), grim, jq, ImageMagick, rsvg-convert.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
PY=${PY:-python3}
samples=${TMPDIR:-/tmp}/kantor-site-samples
"$PY" -I "$here/make_samples.py" "$samples"
mcp="python3 -I $here/mcp_call.py"
themes=/usr/share/omarchy/themes
for pair in dark:catppuccin light:catppuccin-latte; do
  m=${pair%%:*}; t=$themes/${pair#*:}/colors.toml
  POST="$mcp sheets_select '{\"range\":\"B4:G11\"}' >/dev/null" "$here/capture.sh" "$t" "sheets-$m" "$here/shots" "$samples/budget.xlsx"
  POST=: WAIT=8 "$here/capture.sh" "$t" "docs-$m" "$here/shots" "$samples/letter.docx"
  POST="$mcp slides_go_to '{\"slide\":2}' >/dev/null" WAIT=8 "$here/capture.sh" "$t" "slides-$m" "$here/shots" "$samples/workshop.pptx"
done
"$here/images.sh"
"$here/og.sh"
