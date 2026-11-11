#!/usr/bin/env bash
# CI-only bundle: leaves the local signing routine untouched.
set -euo pipefail
version="${1:?version}"
: "${LOUPE_CI_SIGNING_KEYCHAIN:?Release signing keychain is required}"
app=dist/Loupe.app
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cargo build --release --bin loupe
cp target/release/loupe "$app/Contents/MacOS/loupe"
cp assets/Loupe.icns "$app/Contents/Resources/Loupe.icns"
# The .icns is built on Linux by scripts/make-icon.py: have macOS read it back.
iconset="$(mktemp -d)/Loupe.iconset"
iconutil -c iconset "$app/Contents/Resources/Loupe.icns" -o "$iconset" \
  || { echo "iconutil cannot read Loupe.icns" >&2; exit 1; }
for size in 16 32 128 256 512; do
  for suffix in "" "@2x"; do
    name="icon_${size}x${size}${suffix}.png"
    want=$size
    [[ -z "$suffix" ]] || want=$((size * 2))
    got="$(sips -g pixelWidth "$iconset/$name" 2>/dev/null | awk '/pixelWidth/ {print $2}' || true)"
    [[ "$got" == "$want" ]] || {
      echo "Loupe.icns: $name is ${got:-missing}, expected $want px; iconutil wrote:" >&2
      ls -l "$iconset" >&2
      exit 1
    }
  done
done
cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>dev.loupe.app</string>
<key>CFBundleName</key><string>Loupe</string>
<key>CFBundleExecutable</key><string>loupe</string>
<key>CFBundleIconFile</key><string>Loupe</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleShortVersionString</key><string>$version</string>
<key>CFBundleVersion</key><string>${version%%-*}</string>
<key>LSMinimumSystemVersion</key><string>12.0</string>
<key>NSHighResolutionCapable</key><true/>
</dict></plist>
PLIST
codesign -f -s loupe-release --keychain "$LOUPE_CI_SIGNING_KEYCHAIN" -i dev.loupe.app "$app"
codesign --verify --deep --strict "$app"
ditto -c -k --keepParent "$app" dist/loupe-macos-aarch64.zip
