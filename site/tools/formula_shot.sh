#!/usr/bin/env bash
# README screenshot of a formula being edited, with its references coloured in the cell and
# outlined on the grid (dark and light). Run after shots.sh (it uses its sales.xlsx).
# Kantor runs in a private Xwayland server on a private headless output; key presses go
# through that X server only, so nothing touches your screen, focus or keyboard.
# Needs: Xwayland, xdotool, ImageMagick (import, magick), jq.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
bin=${KANTOR_BIN:-$here/../../target/release/kantor}
samples=${SAMPLES:-$HOME/.cache/kantor-site-samples}
dest=${DEST:-$here/../../docs/images}
themes=/usr/share/omarchy/themes
disp=${XDISP:-:97}
mkdir -p "$dest"

out=KFORM
hyprctl output create headless "$out" >/dev/null
xpid=""
cleanup() {
  [ -n "$xpid" ] && kill "$xpid" 2>/dev/null || true
  hyprctl output remove "$out" >/dev/null
}
trap cleanup EXIT
hyprctl eval "hl.monitor({ output = '$out', mode = '1600x1000@60', position = '0x6000', scale = 1 })" >/dev/null
sleep 1
ws=$(hyprctl monitors -j | jq -r ".[] | select(.name==\"$out\") | .activeWorkspace.id")
hyprctl eval "hl.exec_cmd('Xwayland $disp -geometry 1550x975 -noreset', { workspace = '$ws silent' })" >/dev/null
for _ in $(seq 1 40); do DISPLAY=$disp xdotool getdisplaygeometry >/dev/null 2>&1 && break; sleep 0.25; done
xpid=$(pgrep -f "^Xwayland $disp " | head -1)

for pair in dark:catppuccin light:catppuccin-latte; do
  m=${pair%%:*}
  root=${TMPDIR:-/tmp}/kantor-formula-$m
  rm -rf "$root"; mkdir -p "$root/config" "$root/state/omarchy/current/theme" "$root/sock"
  cp "$themes/${pair#*:}/colors.toml" "$root/state/omarchy/current/theme/colors.toml"
  echo 1 > "$root/state/omarchy/current/theme.generation"
  mkdir -p "$root/config/kantor"; printf '[update]\nenabled = false\n' > "$root/config/kantor/settings.toml"
  env -u WAYLAND_DISPLAY DISPLAY=$disp GPUI_X11_SCALE_FACTOR=1.25 XDG_CONFIG_HOME="$root/config" \
    XDG_STATE_HOME="$root/state" KANTOR_SOCKET_DIR="$root/sock" "$bin" "$samples/sales.xlsx" &
  kpid=$!
  win=""
  for _ in $(seq 1 80); do
    win=$(DISPLAY=$disp xdotool search --onlyvisible --class kantor 2>/dev/null | head -1) && [ -n "$win" ] && break
    sleep 0.25
  done
  [ -n "$win" ] || { echo "no window"; kill "$kpid"; exit 1; }
  DISPLAY=$disp xdotool windowmove "$win" 0 0 windowsize "$win" 1550 975
  sleep 6
  KANTOR_SOCKET_DIR=$root/sock XDG_RUNTIME_DIR=$root/sock KANTOR_BIN=$bin \
    python3 -I "$here/mcp_call.py" sheets_select '{"range":"H15"}' >/dev/null
  DISPLAY=$disp xdotool windowfocus --sync "$win"
  sleep 0.5
  # Sales of the cities above target, as a share of all sales: three coloured ranges.
  DISPLAY=$disp xdotool type --delay 60 '=SUMIF(H5:H12,">0",F5:F12)/F13'
  sleep 1.5
  DISPLAY=$disp import -window root "$root/shot.png"
  DISPLAY=$disp xdotool key Escape
  kill "$kpid" 2>/dev/null || true; sleep 0.5; kill -9 "$kpid" 2>/dev/null || true
  magick "$root/shot.png" -crop 1550x975+0+0 +repage -strip -quality 82 "$dest/sheets-formula-$m.webp"
  echo "sheets-formula-$m"
done
