#!/usr/bin/env bash
# Turn the raw window captures in tools/shots/ into the site's images (AVIF, WebP, PNG; 2 widths).
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd); out=$here/../src/img
mkdir -p "$out"
for f in "$here"/shots/*.png; do
  n=$(basename "$f" .png)
  for w in 1550 800; do
    magick "$f" -resize "${w}x" -strip -quality 82 "$out/$n-$w.webp"
    magick "$f" -resize "${w}x" -strip -quality 55 "$out/$n-$w.avif"
    magick "$f" -resize "${w}x" -strip -define png:compression-level=9 +dither -colors 256 "$out/$n-$w.png"
  done
done
