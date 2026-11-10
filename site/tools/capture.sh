#!/usr/bin/env bash
# Capture a real Kantor window on a Hyprland workspace (Hyprland 0.56+, Lua config).
# usage: capture.sh <omarchy-colors.toml> <tag> <out-dir> [absolute path to a file]
# Env: KANTOR_BIN (default: target/release/kantor of this repo), WAIT (seconds to load),
#      SIZE ("1240 780" logical px), POST (shell run against the instance before the shot,
#      e.g. an mcp_call.py selection), WS (workspace to use; shots.sh passes the one shown on
#      its own headless output, so nothing changes on your screen). Kantor runs with throwaway
#      XDG_CONFIG_HOME, XDG_STATE_HOME and KANTOR_SOCKET_DIR, so your own settings and session
#      are untouched.
set -euo pipefail
SIZE=${SIZE:-1240 780}
colors=$1 tag=$2 out=$3 file=${4:-}
here=$(cd "$(dirname "$0")" && pwd)
bin=${KANTOR_BIN:-$here/../../target/release/kantor}
export KANTOR_BIN=$bin
work=${TMPDIR:-/tmp}/kantor-site-capture
root=$work/run-$tag
rm -rf "$root"; mkdir -p "$root/config" "$root/state/omarchy/current/theme" "$root/sock"
cp "$colors" "$root/state/omarchy/current/theme/colors.toml"
echo 1 > "$root/state/omarchy/current/theme.generation"
mkdir -p "$root/config/kantor"; printf '[update]\nenabled = false\n' > "$root/config/kantor/settings.toml"
case $file in ""|/*) ;; *) echo "pass an absolute file path" >&2; exit 1 ;; esac
ws=${WS:-name:ksite}

hyprctl eval "hl.exec_cmd('env XDG_CONFIG_HOME=$root/config XDG_STATE_HOME=$root/state KANTOR_SOCKET_DIR=$root/sock $bin $file', { workspace = '$ws silent', float = true, size = '$SIZE' })" >/dev/null

addr=""
for _ in $(seq 1 120); do
  addr=$(hyprctl clients -j | jq -r '.[] | select(.class=="kantor") | .address + " " + (.pid|tostring)' |
    while read -r a p; do grep -qz "XDG_STATE_HOME=$root/state" "/proc/$p/environ" 2>/dev/null && echo "$a"; done | head -1)
  [ -n "$addr" ] && break
  sleep 0.25
done
[ -n "$addr" ] || { echo "no window"; exit 1; }
client() { hyprctl clients -j | jq -r ".[] | select(.address==\"$addr\") | $1"; }
pid=$(client .pid)
sleep "${WAIT:-6}"
finish() { kill "$pid" 2>/dev/null || true; sleep 0.5; kill -9 "$pid" 2>/dev/null || true; }
if [ -n "${POST:-}" ]; then
  KANTOR_SOCKET_DIR=$root/sock XDG_RUNTIME_DIR=$root/sock bash -c "$POST" || true
fi
if [ -n "${WS:-}" ]; then
  # The workspace is visible on a headless output: the window paints without any focus change.
  sleep 1.5
  grim -T "$(client .stableId)" "$out/$tag.png"; finish; echo "$tag"; exit 0
fi
back=$(hyprctl activeworkspace -j | jq -r '.name')
hyprctl dispatch "hl.dsp.focus({ workspace = 'name:ksite' })" >/dev/null; sleep 1.2
grim -T "$(client .stableId)" "$out/$tag.png"
hyprctl dispatch "hl.dsp.focus({ workspace = '$back' })" >/dev/null
finish; echo "$tag"
