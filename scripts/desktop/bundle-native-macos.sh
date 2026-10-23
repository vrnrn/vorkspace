#!/usr/bin/env bash
# Package an already-built native executable; do not build or launch the app.
# Development bundles use separate data; release bundles use installed app data.
set -euo pipefail
cd "$(dirname "$0")/../.."
if [ "$(uname -s)" != Darwin ]; then
  echo 'macOS app bundles must be packaged on macOS.' >&2
  exit 1
fi

native_profile="${1:-release}"
native_target="${2:-${CARGO_TARGET_DIR:-target}}/$native_profile"
native_version="${3:-$(sed -n 's/^version = "\([^"]*\)".*/\1/p' Cargo.toml)}"
if [ "$native_profile" = release ]; then
  native_data_mode="${4:-vorkspace-data}"
else
  native_data_mode="${4:-dev-data}"
fi
case "$native_data_mode" in
  dev-data|vorkspace-data) ;;
  *) echo "Unknown data mode: $native_data_mode" >&2; exit 1 ;;
esac
native_bundle="$native_target/vorkspace.app"
native_binary="$native_target/vorkspace-gpui"
if [ ! -f "$native_binary" ]; then
  echo "Missing native executable: $native_binary" >&2
  echo 'Build it with cargo first, or use npm run build.' >&2
  exit 1
fi

mkdir -p "$native_bundle/Contents/MacOS" "$native_bundle/Contents/Resources"
# Replace the inode atomically so macOS does not reuse an older signature cache.
cp "$native_binary" "$native_bundle/Contents/MacOS/vorkspace-gpui.next"
mv -f "$native_bundle/Contents/MacOS/vorkspace-gpui.next" "$native_bundle/Contents/MacOS/vorkspace-gpui"
cp crates/vorkspace-gpui/assets/icon.icns "$native_bundle/Contents/Resources/vorkspace.icns"
# Ship the app's attribution and the license texts for GPUI and embedded assets.
# Add these before signing so the signature covers the notices too.
native_licenses="$native_bundle/Contents/Resources/licenses"
mkdir -p "$native_licenses"
cp LICENSE NOTICE licenses/LICENSE-GPUI-APACHE licenses/LICENSE-UPSTREAM-MIT "$native_licenses/"
cp crates/vorkspace-gpui/assets/hugeicons.LICENSE \
  crates/vorkspace-gpui/assets/dark-plus.LICENSE \
  crates/vorkspace-gpui/assets/native-editor.LICENSE.txt "$native_licenses/"
cp crates/vorkspace-gpui/assets/material/LICENSE "$native_licenses/material-icons.LICENSE"
rm -f "$native_licenses/Geist-OFL.txt"
# Launch the binary itself: an exec'ing launcher changes the pid version, and macOS
# then refuses the menu bar item. The app reads VorkspaceDataMode from Info.plist.
native_executable="vorkspace-gpui"
rm -f "$native_bundle/Contents/MacOS/vorkspace-launcher"
cat > "$native_bundle/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleExecutable</key><string>$native_executable</string>
<key>VorkspaceDataMode</key><string>$native_data_mode</string>
<key>CFBundleIdentifier</key><string>com.vorkspace.gpui</string>
<key>CFBundleName</key><string>vorkspace</string>
<key>CFBundleDisplayName</key><string>vorkspace</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleShortVersionString</key><string>$native_version</string>
<key>CFBundleVersion</key><string>$native_version</string>
<key>CFBundleIconFile</key><string>vorkspace.icns</string>
<key>NSPrincipalClass</key><string>NSApplication</string>
<key>NSHighResolutionCapable</key><true/>
</dict></plist>
PLIST
# Release configuration is explicit; never attach development data to a feed.
python3 scripts/desktop/native-bundle-plist.py "$native_bundle/Contents/Info.plist"
if [ -n "${VORKSPACE_SPARKLE_FRAMEWORK:-}" ]; then
  mkdir -p "$native_bundle/Contents/Frameworks"
  # Framework resources can change between files and directories across SDKs.
  # Replace the generated copy instead of overlaying incompatible layouts.
  rm -rf "$native_bundle/Contents/Frameworks/Sparkle.framework"
  ditto "$VORKSPACE_SPARKLE_FRAMEWORK" "$native_bundle/Contents/Frameworks/Sparkle.framework"
  cp "$(dirname "$VORKSPACE_SPARKLE_FRAMEWORK")/LICENSE" "$native_licenses/Sparkle-LICENSE"
else
  rm -rf "$native_bundle/Contents/Frameworks/Sparkle.framework"
fi
native_identity="${VORKSPACE_SIGNING_IDENTITY:--}"
if [ "$native_identity" = - ]; then
  codesign --force --sign - "$native_bundle"
else
  # Sparkle contains nested executables; sign from the leaves toward the root.
  python3 scripts/desktop/sign-native-bundle.py "$native_bundle" "$native_identity"
fi
codesign --verify --deep --strict "$native_bundle"
printf 'App: %s\n' "$native_bundle"
