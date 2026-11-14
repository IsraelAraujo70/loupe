#!/bin/sh
set -eu
pid=$1; target=$2; stage=$3; source=$4; platform=$5; tag=$6; notice=$7
# Lets the new version tell a live helper from one killed mid-install.
echo $$ >"$stage/helper"
while kill -0 "$pid" 2>/dev/null; do sleep 1; done
backup="$stage/previous"
mv "$target" "$backup"
if ! mv "$source" "$target"; then mv "$backup" "$target"; rm -rf "$stage"; exit 1; fi
# The new version writes $stage/healthy once its first frame is on screen. Until
# then the previous version stays in $backup. `open -W` lives as long as the app.
if [ "$platform" = macos ]; then
  /usr/bin/open -W "$target" &
else
  "$target" >"$stage/restart.log" 2>&1 &
fi
child=$!
while [ ! -e "$stage/healthy" ]; do
  if ! kill -0 "$child" 2>/dev/null && [ ! -e "$stage/healthy" ]; then
    mv "$target" "$source"; mv "$backup" "$target"
    printf '%s\n' "$tag" >"$notice.tmp" && mv "$notice.tmp" "$notice" || true
    rm -rf "$stage"
    if [ "$platform" = macos ]; then /usr/bin/open "$target"; else "$target" >/dev/null 2>&1 & fi
    exit 1
  fi
  sleep 1
done
rm -f "$notice"
rm -rf "$stage"
