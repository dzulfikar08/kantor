#!/usr/bin/env bash
# k.sh VERB ARGS: one step against the running Kantor on :33
disp=:33; root=${WORK:-$HOME/.cache/kantor-site-paparan}/run; out=${WORK:-$HOME/.cache/kantor-site-paparan}/out
verb=$1; shift; arg="$*"
case "$verb" in
  key) DISPLAY=$disp xdotool key --delay 80 $arg;;
  type) DISPLAY=$disp xdotool type --delay 25 "$arg";;
  click) DISPLAY=$disp xdotool mousemove $arg click 1;;
  move) DISPLAY=$disp xdotool mousemove $arg;;
  focus) DISPLAY=$disp xdotool windowfocus --sync "$(cat $root/win)";;
  cmd) DISPLAY=$disp xdotool key ctrl+k; sleep 0.6; DISPLAY=$disp xdotool type --delay 30 "$arg"; sleep 0.6; DISPLAY=$disp xdotool key Return;;
  theme) t=catppuccin; [ "$arg" = light ] && t=catppuccin-latte
     cp /usr/share/omarchy/themes/$t/colors.toml "$root/state/omarchy/current/theme/colors.toml"
     echo $(( $(cat "$root/state/omarchy/current/theme.generation") + 1 )) > "$root/state/omarchy/current/theme.generation";;
  shot) DISPLAY=$disp import -window root "$root/s.png"; magick "$root/s.png" -crop 1550x975+0+0 +repage -strip "$out/$arg.png"; echo "$out/$arg.png";;
  stop) p=$(DISPLAY=$disp xdotool search --class kantor getwindowpid 2>/dev/null | head -1); [ -n "$p" ] && kill $p; sleep 1; DISPLAY=$disp xdotool search --class kantor >/dev/null && echo still-open || echo stopped;;
esac
