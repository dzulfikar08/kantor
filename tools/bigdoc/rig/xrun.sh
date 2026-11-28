#!/usr/bin/env bash
# xrun.sh BIN FILE STEPS OUTLOG [ENV=VAL…]
# Runs Kantor (KANTOR_PERF=1) on a copy of FILE in Xwayland :51, waits for
# "[docs] open pages final", then plays STEPS:
#   key <keys>          xdotool key, 400 ms after
#   keys <n> <key> <ms> press <key> n times, <ms> apart
#   type <ms> <text>    type text, <ms> between characters
#   click <x> <y>       left click (window px)
#   mark <text>         echo a marker line into the log
#   sleep <s>
#   waitlog <text>      wait (≤ 600 s) for a log line containing text
#   shot <name>         screenshot next to OUTLOG
#   mem                 log VmRSS/VmHWM
#   settle              wait until no input has reached the view for 10 s (≤ 900 s)
set -uo pipefail
disp=:51
bin=$1; file=$2; steps=$3; log=$4; shift 4
root=${BIGDOC_RUN:-$HOME/.cache/bigdoc/rig/run}
rm -rf "$root"; mkdir -p "$root/config/kantor" "$root/state/omarchy/current/theme" "$root/sock" "$root/doc" "$root/data" "$root/cache"
cp /usr/share/omarchy/themes/catppuccin-latte/colors.toml "$root/state/omarchy/current/theme/colors.toml" 2>/dev/null || true
echo 1 > "$root/state/omarchy/current/theme.generation"
printf '[update]\nenabled = false\n' > "$root/config/kantor/settings.toml"
work="$root/doc/$(basename "$file")"
cp "$file" "$work"
: > "$log"
env -u WAYLAND_DISPLAY KANTOR_PERF=1 DISPLAY=$disp GPUI_X11_SCALE_FACTOR=1 XDG_CONFIG_HOME="$root/config" \
  XDG_STATE_HOME="$root/state" XDG_DATA_HOME="$root/data" KANTOR_SOCKET_DIR="$root/sock" "$@" "$bin" "$work" >> "$log" 2>&1 &
kpid=$!
echo "[rig] pid $kpid file $file bin $bin" >> "$log"
win=""
for _ in $(seq 1 240); do
  win=$(DISPLAY=$disp xdotool search --onlyvisible --class kantor 2>/dev/null | head -1) && [ -n "$win" ] && break
  sleep 0.25
done
[ -n "$win" ] || { echo "no window"; kill "$kpid"; exit 1; }
DISPLAY=$disp xdotool windowmove "$win" 0 0 windowsize "$win" 1600 1000
waitlog() { for _ in $(seq 1 2400); do grep -q -- "$1" "$log" && return 0; sleep 0.25; done; echo "[rig] timeout waiting for $1" >> "$log"; }
waitlog "open pages final"
sleep 2
DISPLAY=$disp xdotool windowfocus --sync "$win"
sleep 0.5
while IFS= read -r line || [ -n "$line" ]; do
  [ -z "$line" ] && continue
  case "$line" in \#*) continue;; esac
  verb=${line%% *}; arg=${line#* }
  case "$verb" in
    key) DISPLAY=$disp xdotool key $arg; sleep 0.4;;
    keys) set -- $arg; for _ in $(seq 1 $1); do DISPLAY=$disp xdotool key $2; sleep $(echo "$3/1000" | bc -l); done; sleep 0.5;;
    type) ms=${arg%% *}; text=${arg#* }; DISPLAY=$disp xdotool type --delay "$ms" "$text"; sleep 1;;
    click) DISPLAY=$disp xdotool mousemove $arg click 1; sleep 0.5;;
    mark) echo "[rig] mark $arg" >> "$log";;
    sleep) sleep "$arg";;
    waitlog) waitlog "$arg";;
    shot) sleep 0.5; DISPLAY=$disp import -window root "${log%.log}-$arg.png";;
    settle) last=-1; same=0; for _ in $(seq 1 3600); do n=$(grep -c "\[docs\] input\|\[docs\] open pages final\|docs save" "$log"); if [ "$n" = "$last" ]; then same=$((same+1)); [ $same -ge 40 ] && break; else same=0; last=$n; fi; sleep 0.25; done;;
    mem) grep -E 'VmRSS|VmHWM' /proc/$kpid/status | tr '\n' ' ' | sed 's/^/[rig] mem /' >> "$log"; echo >> "$log";;
  esac
done < "$steps"
kill "$kpid" 2>/dev/null; sleep 0.5; kill -9 "$kpid" 2>/dev/null
true
