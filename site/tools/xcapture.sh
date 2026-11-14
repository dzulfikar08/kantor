#!/usr/bin/env bash
# Capture a Kantor window that needs key presses or a pointer before the shot (scrolling a
# document, hovering a note). Kantor runs in a private Xwayland server (XDISP, started by
# shots.sh on its headless output), so keys and the pointer never reach your own session.
# usage: xcapture.sh <omarchy-colors.toml> <tag> <out-dir> <absolute path to a file>
# Env: KANTOR_BIN, XDISP (X display, e.g. :96), WAIT (seconds to load), POST (shell run against
#      the instance before STEPS, e.g. an mcp_call.py selection), STEPS (lines of xdotool
#      arguments run in order; "sleep N" pauses).
set -euo pipefail
colors=$1 tag=$2 out=$3 file=$4
here=$(cd "$(dirname "$0")" && pwd)
bin=${KANTOR_BIN:-$here/../../target/release/kantor}
export KANTOR_BIN=$bin
disp=${XDISP:?start an Xwayland and set XDISP}
root=${TMPDIR:-/tmp}/kantor-site-capture/x-$tag
rm -rf "$root"; mkdir -p "$root/config/kantor" "$root/state/omarchy/current/theme" "$root/sock"
cp "$colors" "$root/state/omarchy/current/theme/colors.toml"
echo 1 > "$root/state/omarchy/current/theme.generation"
printf '[update]\nenabled = false\n' > "$root/config/kantor/settings.toml"

env -u WAYLAND_DISPLAY DISPLAY="$disp" GPUI_X11_SCALE_FACTOR=1.25 XDG_CONFIG_HOME="$root/config" \
  XDG_STATE_HOME="$root/state" KANTOR_SOCKET_DIR="$root/sock" "$bin" "$file" >"$root/log" 2>&1 &
kpid=$!
finish() { kill "$kpid" 2>/dev/null || true; sleep 0.5; kill -9 "$kpid" 2>/dev/null || true; }
trap finish EXIT
win=""
for _ in $(seq 1 80); do
  win=$(DISPLAY=$disp xdotool search --onlyvisible --class kantor 2>/dev/null | head -1) && [ -n "$win" ] && break
  sleep 0.25
done
[ -n "$win" ] || { echo "no window"; exit 1; }
DISPLAY=$disp xdotool windowmove "$win" 0 0 windowsize "$win" 1550 975
sleep "${WAIT:-6}"
if [ -n "${POST:-}" ]; then
  KANTOR_SOCKET_DIR=$root/sock XDG_RUNTIME_DIR=$root/sock bash -c "$POST" || true
fi
DISPLAY=$disp xdotool windowfocus --sync "$win"
sleep 0.5
while IFS= read -r step; do
  [ -z "$step" ] && continue
  case $step in
    sleep\ *) sleep "${step#sleep }" ;;
    *) eval "DISPLAY=$disp xdotool $step" ;;
  esac
done <<< "${STEPS:-}"
sleep 1.5
DISPLAY=$disp import -window root "$root/shot.png"
magick "$root/shot.png" -crop 1550x975+0+0 +repage "$out/$tag.png"
echo "$tag"
