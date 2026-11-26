#!/usr/bin/env bash
# kstart.sh MODE(dark|light) [LANG]: start Kantor on the private X server :33 with throwaway config,
# state and data dirs, a private D-Bus session and the fake file-chooser portal (portal.py answers
# every Open/Save with $FAKE_SAVE_PATH). The provider is Ollama and no account is used: the chat
# runs a script (CHAT_SCRIPT), so nothing is sent anywhere.
set -uo pipefail
here=$(cd "$(dirname "$0")" && pwd)
disp=:33
mode=$1; lang=${2:-en}
bin=${KANTOR_BIN:-$(command -v kantor)}
themes=/usr/share/omarchy/themes
theme=catppuccin; [ "$mode" = light ] && theme=catppuccin-latte
root=${WORK:-$HOME/.cache/kantor-site-paparan}/run
rm -rf "$root"; mkdir -p "$root/config/kantor" "$root/state/omarchy/current/theme" "$root/sock" "$root/data"
cp "$themes/$theme/colors.toml" "$root/state/omarchy/current/theme/colors.toml"
echo 1 > "$root/state/omarchy/current/theme.generation"
printf '[ai]\nenabled = true\nprovider = "%s"\n\n[update]\nenabled = false\n' "${PROVIDER:-ollama}" > "$root/config/kantor/settings.toml"
cat > "$root/inner.sh" <<IN
python3 -I $here/portal.py > "$root/portal.log" 2>&1 &
sleep 1
exec env -u WAYLAND_DISPLAY DISPLAY=$disp GPUI_X11_SCALE_FACTOR=1.25 XDG_CONFIG_HOME="$root/config" \
  XDG_STATE_HOME="$root/state" XDG_DATA_HOME="$root/data" KANTOR_SOCKET_DIR="$root/sock" \
  KANTOR_CHAT_SCRIPT="${CHAT_SCRIPT:-}" KANTOR_LANG="$lang" "$bin"
IN
FAKE_SAVE_PATH=${FAKE_SAVE_PATH:-/dev/null} setsid dbus-run-session -- bash "$root/inner.sh" > "$root/log.txt" 2>&1 &
echo $! > "$root/pid"
win=""
for _ in $(seq 1 120); do
  win=$(DISPLAY=$disp xdotool search --onlyvisible --class kantor 2>/dev/null | head -1) && [ -n "$win" ] && break
  sleep 0.25
done
[ -n "$win" ] || { echo "no window"; exit 1; }
# A window that has only just mapped can ignore the first resize: repeat until it sticks.
for _ in $(seq 1 20); do
  DISPLAY=$disp xdotool windowmove "$win" 0 0 windowsize "$win" 1550 975
  sleep 0.5
  DISPLAY=$disp xdotool getwindowgeometry "$win" | grep -q "Geometry: 1550x975" && break
done
echo "$win" > "$root/win"
sleep 4
DISPLAY=$disp xdotool windowfocus --sync "$win"
echo started
