#!/usr/bin/env bash
# Screenshots a Loupe page for UX review on Linux, on an X display of its own.
#
#   scripts/ux-shot.sh [options] <out-prefix>
#
#   --bin PATH      debug binary (default target/debug/loupe; release ignores the overrides)
#   --open URL      page to open: loupe://pr/<owner>/<repo>/<n> or a github.com PR link
#   --tab NAME      pull request tab: conversation, guide, files, commits, checks
#   --size WxH      window size (default 1916x1030)
#   --theme NAME    Dark or Light (default Dark)
#   --data DIR      start from a copy of this data directory (keeps generated guides)
#   --wait SECONDS  time to let the page load (default 20)
#   --pages N       scroll and take up to N consecutive shots, stopping at the end (default 1)
#   --display N     X display number (default: the first free one from 160)
#
# Writes <out-prefix>-p01.png, -p02.png, ... The GitHub token comes from
# GITHUB_TOKEN or `gh auth token`. Needs Xvfb, dbus-daemon, xdotool and
# ImageMagick. Stops only the processes it started.
set -euo pipefail

bin=target/debug/loupe open="" tab="" size=1916x1030 theme=Dark data="" wait=20 pages=1 display=""
while [ $# -gt 1 ]; do
  case $1 in
    --bin) bin=$2 ;;
    --open) open=$2 ;;
    --tab) tab=$2 ;;
    --size) size=$2 ;;
    --theme) theme=$2 ;;
    --data) data=$2 ;;
    --wait) wait=$2 ;;
    --pages) pages=$2 ;;
    --display) display=$2 ;;
    *) echo "unknown option $1" >&2; exit 2 ;;
  esac
  shift 2
done
out=${1:?usage: scripts/ux-shot.sh [options] <out-prefix>}
bin=$(realpath "$bin")
mkdir -p "$(dirname "$out")"

if [ -z "$display" ]; then
  display=160
  while [ -e /tmp/.X11-unix/X$display ]; do display=$((display + 1)); done
fi

run=$(mktemp -d)
chmod 700 "$run"
pids=()
cleanup() {
  for pid in "${pids[@]}"; do kill "$pid" 2>/dev/null || true; done
  rm -rf "$run"
}
trap cleanup EXIT

mkdir -p "$run/data" "$run/xdg"
chmod 700 "$run/xdg"
[ -n "$data" ] && cp -r "$data/." "$run/data/"
python3 - "$run/data/settings.json" "$theme" <<'EOF'
import json, os, sys
path, theme = sys.argv[1], sys.argv[2]
settings = json.load(open(path)) if os.path.exists(path) else {}
settings.setdefault("appearance", {})["color_scheme"] = theme
json.dump(settings, open(path, "w"))
EOF

export XDG_RUNTIME_DIR=$run/xdg
Xvfb :$display -screen 0 3840x2160x24 -nolisten tcp >"$run/xvfb.log" 2>&1 &
pids+=($!)
sleep 1
export DISPLAY=:$display
dbus-daemon --session --fork --print-address=3 --print-pid=4 3>"$run/bus.addr" 4>"$run/bus.pid"
pids+=($(cat "$run/bus.pid"))
export DBUS_SESSION_BUS_ADDRESS=$(cat "$run/bus.addr")

token=${GITHUB_TOKEN:-$(gh auth token)}
LOUPE_DATA_DIR=$run/data LOUPE_DESKTOP_ENTRY=0 GITHUB_TOKEN=$token \
  LOUPE_DEBUG_WINDOW=$size LOUPE_DEBUG_OPEN=$open LOUPE_DEBUG_PR_TAB=$tab \
  "$bin" >"$run/app.log" 2>&1 &
pids+=($!)

window=""
for _ in $(seq 1 30); do
  window=$(xdotool search --name '^Loupe' 2>/dev/null | head -1 || true)
  [ -n "$window" ] && break
  sleep 1
done
[ -n "$window" ] || { echo "the Loupe window never appeared; log:" >&2; cat "$run/app.log" >&2; exit 1; }
xdotool windowmove "$window" 0 0
sleep "$wait"

width=${size%x*} height=${size#*x}
previous=""
for page in $(seq 1 "$pages"); do
  xdotool mousemove --window "$window" $((width - 4)) $((height - 4))
  sleep 0.5
  shot=$(printf '%s-p%02d.png' "$out" "$page")
  import -window "$window" "$shot"
  hash=$(convert "$shot" ppm:- | md5sum | cut -c1-16)
  if [ "$hash" = "$previous" ]; then rm "$shot"; break; fi
  previous=$hash
  echo "$shot"
  xdotool mousemove --window "$window" $((width / 2)) $((height / 2))
  for _ in $(seq 1 10); do xdotool click 5; sleep 0.03; done
  sleep 0.8
done
