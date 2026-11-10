#!/usr/bin/env bash
# Capture a real Kantor window on a private Hyprland workspace (Hyprland 0.56+, Lua config).
# usage: capture.sh <omarchy-colors.toml> <tag> <out-dir> [absolute path to a file]
# Env: KANTOR_BIN (default: target/release/kantor of this repo), WAIT (seconds to load),
#      SIZE ("1240 780" logical px), POST (shell run against the instance before the shot,
#      e.g. an mcp_call.py selection). Kantor runs with throwaway XDG_CONFIG_HOME,
#      XDG_STATE_HOME and KANTOR_SOCKET_DIR, so your own settings and session are untouched.
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
case $file in ""|/*) ;; *) echo "pass an absolute file path" >&2; exit 1 ;; esac

back=$(hyprctl activeworkspace -j | jq -r '.name')
hyprctl eval "hl.exec_cmd('env XDG_CONFIG_HOME=$root/config XDG_STATE_HOME=$root/state KANTOR_SOCKET_DIR=$root/sock $bin $file', { workspace = 'name:ksite silent', float = true, size = '$SIZE' })" >/dev/null

addr=""
for _ in $(seq 1 60); do
  addr=$(hyprctl clients -j | jq -r '.[] | select(.workspace.name=="ksite" and .class=="kantor") | .address' | head -1)
  [ -n "$addr" ] && break
  sleep 0.25
done
[ -n "$addr" ] || { echo "no window"; exit 1; }
pid=$(hyprctl clients -j | jq -r ".[] | select(.address==\"$addr\") | .pid")
sleep "${WAIT:-6}"
if [ -n "${POST:-}" ]; then KANTOR_SOCKET_DIR=$root/sock XDG_RUNTIME_DIR=$root/sock bash -c "$POST" || true
  hyprctl dispatch "hl.dsp.focus({ workspace = 'name:ksite' })" >/dev/null; sleep 1.2
  grim -T "$(hyprctl clients -j | jq -r ".[] | select(.address==\"$addr\") | .stableId")" "$out/$tag.png"
  hyprctl dispatch "hl.dsp.focus({ workspace = '$back' })" >/dev/null
  kill "$pid" 2>/dev/null || true; sleep 0.5; kill -9 "$pid" 2>/dev/null || true
  echo "$tag shown"; exit 0
fi
stable=$(hyprctl clients -j | jq -r ".[] | select(.address==\"$addr\") | .stableId")
if [ -z "${GEO:-}" ] && grim -T "$stable" "$out/$tag.png" 2>/dev/null; then
  kill "$pid" 2>/dev/null || true; sleep 0.5; kill -9 "$pid" 2>/dev/null || true
  echo "$tag toplevel"; exit 0
fi

hyprctl dispatch "hl.dsp.focus({ workspace = 'name:ksite' })" >/dev/null
sleep 1.0
geo=$(hyprctl clients -j | jq -r ".[] | select(.address==\"$addr\") | \"\(.at[0]),\(.at[1]) \(.size[0])x\(.size[1])\"")
grim -g "$geo" "$out/$tag.png"
hyprctl dispatch "hl.dsp.focus({ workspace = '$back' })" >/dev/null
kill "$pid" 2>/dev/null || true
sleep 0.5
kill -9 "$pid" 2>/dev/null || true
echo "$tag $geo"
