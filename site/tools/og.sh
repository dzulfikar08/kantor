#!/usr/bin/env bash
# Open Graph card (1200x630) from the Kantor icon and the real Sheets capture.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd); tmp=$(mktemp -d)
F=${OG_FONT:-$(fc-match -f '%{file}' 'Inter')}
rsvg-convert -w 132 -h 132 "$here/../../assets/icons/kantor.svg" -o "$tmp/icon.png"
magick "$here/shots/sheets-dark.png" -resize 760x \( +clone -alpha extract -fill black -colorize 100 -fill white -draw "roundrectangle 0,0 759,477 14,14" \) -alpha off -compose CopyOpacity -composite "$tmp/shot.png"
magick -size 1200x630 gradient:'#232838'-'#0f1219' \
  "$tmp/shot.png" -geometry +560+150 -compose over -composite \
  "$tmp/icon.png" -geometry +60+64 -composite \
  -font "$F" -fill white -pointsize 76 -annotate +64+290 "Kantor" \
  -fill '#c9d1e3' -pointsize 30 -annotate +66+350 "Sheets, Docs and Slides" -annotate +66+392 "for Windows, Mac and Linux" \
  -fill '#93a0bb' -pointsize 24 -annotate +66+470 "Opens and saves Word, Excel" -annotate +66+504 "and PowerPoint files." -annotate +66+560 "Free and open source" \
  -fill '#4f8ff7' -draw "rectangle 66,590 126,596" -fill '#2fbf71' -draw "rectangle 132,590 192,596" -fill '#f47a3a' -draw "rectangle 198,590 258,596" \
  -strip "$here/../src/img/og.png"
rm -rf "$tmp"
