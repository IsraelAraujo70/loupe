#!/usr/bin/env bash
# Build dist/Loupe.app on macOS, preferring the shared Loupe release identity.
#
#   scripts/bundle-macos.sh              build dist/Loupe.app
#   scripts/bundle-macos.sh --install    also copy it to ~/Applications/Loupe.app
#
# Uses only tools macOS ships (plutil, codesign, ditto, rsync) plus cargo.
# Temp files live under dist/. Deletions are of paths inside dist/, and of files
# inside ~/Applications/Loupe.app that the new build no longer has.
set -euo pipefail

BUNDLE_ID="dev.loupe.app"

install=0
for arg in "$@"; do
  case "$arg" in
    --install) install=1 ;;
    -h|--help) sed -n '2,8p' "$0"; exit 0 ;;
    *) echo "unknown option: $arg" >&2; exit 2 ;;
  esac
done

[[ "$(uname -s)" == "Darwin" ]] || { echo "bundle-macos.sh runs on macOS only" >&2; exit 1; }

export PATH="$HOME/.cargo/bin:/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin"
root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

# Version of `[workspace.package]` in Cargo.toml.
version="$(awk '
  /^\[/ { in_section = ($0 == "[workspace.package]") }
  in_section && /^version[[:space:]]*=/ { gsub(/.*=[[:space:]]*"|".*/, ""); print; exit }
' Cargo.toml)"
[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+([-+][0-9A-Za-z.-]+)?$ ]] \
  || { echo "could not read workspace version from Cargo.toml (got '$version')" >&2; exit 1; }
# CFBundleVersion must be numeric dot-separated: drop any pre-release suffix.
numeric_version="${version%%[-+]*}"

dist="$root/dist"
app="$dist/Loupe.app"

echo "==> cargo build --release --bin loupe"
cargo build --release --bin loupe
binary="$root/target/release/loupe"
[[ -x "$binary" ]] || { echo "missing $binary after build" >&2; exit 1; }

mkdir -p "$dist"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"

echo "==> icon"
# Built by scripts/make-icon.py, with a simpler drawing at 16 and 32 px.
cp "$root/assets/Loupe.icns" "$app/Contents/Resources/Loupe.icns"

echo "==> bundle"
cp "$binary" "$app/Contents/MacOS/loupe"
chmod 755 "$app/Contents/MacOS/loupe"

cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleIdentifier</key><string>$BUNDLE_ID</string>
  <key>CFBundleName</key><string>Loupe</string>
  <key>CFBundleDisplayName</key><string>Loupe</string>
  <key>CFBundleExecutable</key><string>loupe</string>
  <key>CFBundleIconFile</key><string>Loupe</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
  <key>CFBundleShortVersionString</key><string>$version</string>
  <key>CFBundleVersion</key><string>$numeric_version</string>
  <key>LSMinimumSystemVersion</key><string>12.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>LSApplicationCategoryType</key><string>public.app-category.developer-tools</string>
  <key>CFBundleURLTypes</key>
  <array>
    <dict>
      <key>CFBundleURLName</key><string>$BUNDLE_ID</string>
      <key>CFBundleURLSchemes</key><array><string>loupe</string></array>
    </dict>
  </array>
</dict>
</plist>
PLIST
plutil -lint "$app/Contents/Info.plist"

signed=0
for config in loupe-release:loupe-signing; do
  identity="${config%%:*}"
  signing_dir="$HOME/.config/${config#*:}"
  signing_keychain="$signing_dir/signing.keychain-db"
  if [[ -f "$signing_keychain" ]] && security find-identity -p codesigning "$signing_keychain" | grep -Fq "\"$identity\""; then
    echo "==> codesign ($identity)"
    security unlock-keychain -p "$(cat "$signing_dir/kc.pass")" "$signing_keychain"
    codesign -f -s "$identity" --keychain "$signing_keychain" -i "$BUNDLE_ID" "$app"
    signed=1
    break
  fi
done
if [[ $signed == 0 ]]; then
  echo "==> codesign (ad-hoc)"
  codesign --force --deep -s - "$app"
fi
codesign -v "$app"

if [[ $install == 1 ]]; then
  target_dir="$HOME/Applications"
  target="$target_dir/Loupe.app"
  mkdir -p "$target_dir"
  if [[ -e "$target" ]]; then
    existing_id="$(plutil -extract CFBundleIdentifier raw -o - "$target/Contents/Info.plist" 2>/dev/null || true)"
    if [[ "$existing_id" != "$BUNDLE_ID" ]]; then
      echo "refusing to replace $target: its CFBundleIdentifier is '${existing_id:-unreadable}', expected $BUNDLE_ID" >&2
      exit 1
    fi
  fi
  # Update the bundle where it is. Moving or deleting the bundle of a running
  # Loupe leaves macOS unable to open new accessibility connections to it, so
  # UI agents and proofs see an empty tree until it restarts.
  rsync -a --delete "$app/" "$target/"
  codesign -v "$target"
  echo "installed: $target"
  running="$(pgrep -f "^$target/Contents/MacOS/loupe" || true)"
  if [[ -n "$running" ]]; then
    echo "still on the previous build until reopened: pid $(echo $running | tr ' ' ',')"
  fi
fi

echo "bundle: $app (version $version)"
