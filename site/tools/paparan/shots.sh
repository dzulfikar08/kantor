#!/usr/bin/env bash
# Re-take the site's Paparan AI captures (paparan-{dark,light}, paparan-id-{dark,light}) and
# convert them to the site's formats. A real Kantor (KANTOR_BIN, 0.25.0 or later) runs the chat
# panel with a scripted model (KANTOR_CHAT_SCRIPT): the turns are fixed, the tools are real, so
# the workbook in the shot is built and checked by Kantor, and no account or credits are used.
# Needs what ../shots.sh needs, plus python3-gobject (portal.py) and dbus-run-session.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
export WORK=${WORK:-$HOME/.cache/kantor-site-paparan}
export KANTOR_BIN=${KANTOR_BIN:-$(command -v kantor)}
mkdir -p "$WORK/out"
python3 -I "$here/make_csv.py" "$WORK/penjualan-2026.csv"
python3 -I "$here/make_book.py" "$WORK/penjualan-2026.csv" "$WORK/book-en.json"
python3 -I "$here/make_book.py" "$WORK/penjualan-2026.csv" "$WORK/book-id.json" id
python3 -I "$here/make_scripts.py" "$WORK/book-en.json" "$WORK/book-id.json" "$WORK"
"$here/xsetup.sh"
trap 'pkill -f "^Xwayland :33 " || true; hyprctl output remove KSPAI >/dev/null' EXIT
# Coordinates are window pixels at 125 %: the Workbook chip, then the Open button of the file card
# (they sit elsewhere in the Indonesian UI).
"$here/take.sh" en "$WORK/script-en.json" \
  "Turn penjualan-2026.csv into a workbook for the finance team: monthly totals per city with live formulas, and a line chart of the monthly trend." paparan
PALETTE="Obrolan Paparan AI" KINDX=1305 OPENX=1326 "$here/take.sh" id "$WORK/script-id.json" \
  "Ubah penjualan-2026.csv jadi buku kerja untuk tim keuangan: total bulanan per kota dengan rumus, dan grafik garis tren bulanannya." paparan-id
img=$here/../../src/img
for n in paparan-dark paparan-light paparan-id-dark paparan-id-light; do
  cp "$WORK/out/$n.png" "$here/../shots/$n.png"
  for w in 1550 800; do
    magick "$WORK/out/$n.png" -resize "${w}x" -strip -quality 82 "$img/$n-$w.webp"
    magick "$WORK/out/$n.png" -resize "${w}x" -strip -quality 55 "$img/$n-$w.avif"
    magick "$WORK/out/$n.png" -resize "${w}x" -strip -define png:compression-level=9 +dither -colors 256 "$img/$n-$w.png"
  done
done
