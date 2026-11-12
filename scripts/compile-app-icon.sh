#!/usr/bin/env bash
# Compile assets/AppIcon.icon (Icon Composer) into Assets.car for macOS 26, with its Light and Dark
# appearances, and check what actool wrote.
#
#   scripts/compile-app-icon.sh <Contents/Resources dir> [--optional]
#
# Needs Xcode 26 (actool, assetutil). With --optional, a Mac without them gets a warning and keeps only
# Loupe.icns; without it, that is an error. actool exits 0 on many failures, so this script checks the
# catalog itself: an "AppIcon" icon with a dark-appearance rendition.
# Info.plist must name the icon: CFBundleIconName = AppIcon (CFBundleIconFile stays Loupe for macOS < 26).
set -euo pipefail

resources="${1:?Contents/Resources directory}"
optional=0
[[ "${2:-}" == "--optional" ]] && optional=1
root="$(cd "$(dirname "$0")/.." && pwd)"

if ! xcrun --find actool >/dev/null 2>&1 || ! xcrun --find assetutil >/dev/null 2>&1; then
  if [[ $optional == 1 ]]; then
    echo "warning: no actool (Xcode 26): the app gets only Loupe.icns, without the macOS 26 Dark icon" >&2
    exit 0
  fi
  echo "actool not found: compiling AppIcon.icon needs Xcode 26" >&2
  exit 1
fi

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
xcrun actool "$root/assets/AppIcon.icon" --compile "$work" \
  --app-icon AppIcon --platform macosx --target-device mac \
  --minimum-deployment-target 26.0 --enable-on-demand-resources NO \
  --development-region en --enable-icon-stack-fallback-generation=disabled \
  --output-partial-info-plist "$work/partial.plist" \
  --output-format human-readable-text --notices --warnings --errors

[[ -s "$work/Assets.car" ]] || { echo "actool wrote no Assets.car" >&2; exit 1; }
info="$(xcrun assetutil --info "$work/Assets.car")"
grep -q '"Name" : "AppIcon"' <<<"$info" || { echo "Assets.car has no AppIcon:" >&2; echo "$info" >&2; exit 1; }
grep -q 'DarkAqua\|"Appearance" : "dark"\|NSAppearanceNameDark' <<<"$info" \
  || { echo "Assets.car has no dark rendition of AppIcon:" >&2; echo "$info" >&2; exit 1; }
echo "AppIcon appearances in Assets.car:"
grep -o '"Appearance" : "[^"]*"' <<<"$info" | sort | uniq -c || true
cp "$work/Assets.car" "$resources/Assets.car"
