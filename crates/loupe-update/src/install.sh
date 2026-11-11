#!/bin/sh
set -eu
pid=$1; target=$2; stage=$3; source=$4; platform=$5
while kill -0 "$pid" 2>/dev/null; do sleep 1; done
backup="$stage/previous"
mv "$target" "$backup"
if ! mv "$source" "$target"; then mv "$backup" "$target"; rm -rf "$stage"; exit 1; fi
if [ "$platform" = macos ]; then
  if ! /usr/bin/open "$target"; then mv "$target" "$source"; mv "$backup" "$target"; /usr/bin/open "$target"; rm -rf "$stage"; exit 1; fi
else
  "$target" >"$stage/restart.log" 2>&1 &
  child=$!
  sleep 3
  if ! kill -0 "$child" 2>/dev/null; then mv "$target" "$source"; mv "$backup" "$target"; "$target" >/dev/null 2>&1 & rm -rf "$stage"; exit 1; fi
fi
rm -rf "$stage"
