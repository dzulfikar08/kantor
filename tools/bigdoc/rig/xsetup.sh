#!/usr/bin/env bash
# A private headless Hyprland output (KBIGD) with Xwayland :51 on it, so
# Kantor paints without showing on (or taking input from) the screen.
set -euo pipefail
disp=:51
out=KBIGD
if ! hyprctl monitors -j | jq -e ".[] | select(.name==\"$out\")" >/dev/null; then
  hyprctl output create headless "$out" >/dev/null
  sleep 0.5
  hyprctl eval "hl.monitor({ output = '$out', mode = '1600x1000@60', position = '0x31000', scale = 1 })" >/dev/null
  sleep 1
fi
ws=$(hyprctl monitors -j | jq -r ".[] | select(.name==\"$out\") | .activeWorkspace.id")
if ! DISPLAY=$disp xdotool getdisplaygeometry >/dev/null 2>&1; then
  hyprctl eval "hl.exec_cmd('Xwayland $disp -geometry 1600x1000 -noreset', { workspace = '$ws silent', float = true, size = '1600 1000' })" >/dev/null
  for _ in $(seq 1 40); do DISPLAY=$disp xdotool getdisplaygeometry >/dev/null 2>&1 && break; sleep 0.25; done
fi
echo "ready ws=$ws"
