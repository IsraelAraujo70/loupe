#!/usr/bin/env bash
# Compile assets/AppIcon.icon (Icon Composer) into Assets.car for macOS 26, with its Light and Dark
# appearances, and check what actool wrote.
#
#   scripts/compile-app-icon.sh <Contents/Resources dir> [--optional]
#
# Needs a macOS 26 host with Xcode 26 (actool, assetutil): on older hosts actool's asset runtime crashes.
# Without --optional any failure is an error (CI). With --optional (local bundles) a missing or failing
# actool is a warning, and the app keeps only Loupe.icns, without the macOS 26 Dark icon.
#
# actool exits 0 on many failures and reports missing inputs as notices, so this script reads its XML
# report and fails on any error, warning, or notice, then reads the catalog: it must hold the AppIcon for
# both the Aqua and the DarkAqua appearance.
# Info.plist must name the icon: CFBundleIconName = AppIcon (CFBundleIconFile stays Loupe for macOS < 26).
set -euo pipefail

resources="${1:?Contents/Resources directory}"
optional=0
[[ "${2:-}" == "--optional" ]] && optional=1
root="$(cd "$(dirname "$0")/.." && pwd)"
icon="$root/assets/AppIcon.icon"

fail() {
  if [[ $optional == 1 ]]; then
    echo "warning: $1; the app gets only Loupe.icns, without the macOS 26 Dark icon" >&2
    exit 0
  fi
  echo "$1" >&2
  exit 1
}

xcrun --find actool >/dev/null 2>&1 && xcrun --find assetutil >/dev/null 2>&1 \
  || fail "actool not found: compiling AppIcon.icon needs Xcode 26"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
xcrun actool "$icon" --compile "$work" \
  --app-icon AppIcon --platform macosx --target-device mac \
  --minimum-deployment-target 26.0 --enable-on-demand-resources NO \
  --development-region en --enable-icon-stack-fallback-generation=disabled \
  --output-partial-info-plist "$work/partial.plist" \
  --output-format xml1 --notices --warnings --errors > "$work/report.plist" \
  || fail "actool failed: $(cat "$work/report.plist" 2>/dev/null)"

python3 - "$work/report.plist" <<'PY' || fail "actool reported problems compiling AppIcon.icon"
import plistlib, sys
report = plistlib.load(open(sys.argv[1], "rb"))
problems = {key: report[key] for key in report if key.endswith((".errors", ".warnings", ".notices")) and report[key]}
if problems:
    for key, items in problems.items():
        for item in items:
            print(f"{key}: {item.get('description', item)}", file=sys.stderr)
    sys.exit(1)
PY

[[ -s "$work/Assets.car" ]] || fail "actool wrote no Assets.car"
xcrun assetutil --info "$work/Assets.car" > "$work/info.json" || fail "assetutil cannot read Assets.car"
python3 - "$work/info.json" <<'PY' || fail "Assets.car does not hold the AppIcon for Light and Dark"
import json, sys
entries = [e for e in json.load(open(sys.argv[1])) if isinstance(e, dict) and e.get("Name") == "AppIcon"]
appearances = sorted({e.get("Appearance", "NSAppearanceNameAqua (default)") for e in entries})
print("AppIcon renditions:", len(entries), "appearances:", ", ".join(appearances) or "none")
if not entries:
    sys.exit("no AppIcon rendition")
if not any("Dark" in a for a in appearances):
    sys.exit("no dark rendition of AppIcon")
PY
cp "$work/Assets.car" "$resources/Assets.car"

# Proof for a person reading the log: Icon Composer's ictool renders the Light and Dark icons, and the
# script prints the colour at a point on the front face's base (ink on Light, paper on Dark). Never fails.
ictool="$(xcode-select -p)/../Applications/Icon Composer.app/Contents/Executables/ictool"
if [[ -x "$ictool" ]]; then
  for rendition in Default Dark; do
    png="$work/AppIcon-$rendition.png"
    if "$ictool" "$icon" --export-image --output-file "$png" --platform macOS --rendition "$rendition" \
        --width 1024 --height 1024 --scale 1 >/dev/null 2>&1 && sips -s format bmp "$png" --out "$png.bmp" >/dev/null 2>&1; then
      python3 - "$png.bmp" "$rendition" <<'PY' || true
import struct, sys
data = open(sys.argv[1], "rb").read()
offset, = struct.unpack_from("<I", data, 10)
width, height, _, bpp = struct.unpack_from("<iiHH", data, 18)
def pixel(x, y):
    row = (abs(height) - 1 - y) if height > 0 else y
    i = offset + row * ((width * bpp // 8 + 3) & ~3) + x * bpp // 8
    b, g, r = data[i:i + 3]
    return r, g, b
x, y = 458 * width // 1024, 667 * abs(height) // 1024  # the front face's base, as make-icon.py draws it
r, g, b = pixel(x, y)
print(f"ictool {sys.argv[2]}: {width}x{abs(height)}, front face at ({x},{y}) = #{r:02x}{g:02x}{b:02x}, luma {round(0.2126*r + 0.7152*g + 0.0722*b)}")
PY
    else
      echo "note: ictool could not render the $rendition icon" >&2
    fi
  done
else
  echo "note: no Icon Composer ictool at $ictool, no preview" >&2
fi
