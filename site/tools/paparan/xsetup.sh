#!/usr/bin/env bash
# A private headless Hyprland output (KSPAI) with an Xwayland server :33 on it, so Kantor can be
# clicked and typed into without touching your screen, keyboard or focus.
set -euo pipefail
disp=:33
out=KSPAI
if ! hyprctl monitors -j | jq -e ".[] | select(.name==\"$out\")" >/dev/null; then
  hyprctl output create headless "$out" >/dev/null
  sleep 0.5
  hyprctl eval "hl.monitor({ output = '$out', mode = '1600x1000@60', position = '0x19000', scale = 1 })" >/dev/null
  sleep 1
fi
ws=$(hyprctl monitors -j | jq -r ".[] | select(.name==\"$out\") | .activeWorkspace.id")
if ! DISPLAY=$disp xdotool getdisplaygeometry >/dev/null 2>&1; then
  hyprctl eval "hl.exec_cmd('Xwayland $disp -geometry 1550x975 -noreset', { workspace = '$ws silent', float = true, size = '1600 1000', move = '0 0' })" >/dev/null
  for _ in $(seq 1 40); do DISPLAY=$disp xdotool getdisplaygeometry >/dev/null 2>&1 && break; sleep 0.25; done
fi
echo "ready ws=$ws"
