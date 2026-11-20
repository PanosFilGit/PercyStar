#!/usr/bin/env bash
# Builds dist/PercyStar.app: universal (arm64 + x86_64), fat LTO, CPU-tuned per arch, ad-hoc signed.
set -euo pipefail
cd "$(dirname "$0")/.."

APP=dist/PercyStar.app
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
export MACOSX_DEPLOYMENT_TARGET=11.0

rustup target add aarch64-apple-darwin x86_64-apple-darwin >/dev/null

# apple-m1 tuning runs on every Apple Silicon Mac; x86-64-v3 (Haswell+) covers every Intel Mac that runs macOS 11+
RUSTFLAGS="-C target-cpu=apple-m1" cargo build --release --target aarch64-apple-darwin
RUSTFLAGS="-C target-cpu=x86-64-v3" cargo build --release --target x86_64-apple-darwin

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
lipo -create -output "$APP/Contents/MacOS/PercyStar" \
  target/aarch64-apple-darwin/release/PercyStar \
  target/x86_64-apple-darwin/release/PercyStar

ICONSET=$(mktemp -d)/PercyStar.iconset
mkdir -p "$ICONSET"
for s in 16 32 128 256 512; do
  sips -z $s $s assets/logo.png --out "$ICONSET/icon_${s}x${s}.png" >/dev/null
  sips -z $((s * 2)) $((s * 2)) assets/logo.png --out "$ICONSET/icon_${s}x${s}@2x.png" >/dev/null
done
iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/PercyStar.icns"

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>PercyStar</string>
  <key>CFBundleDisplayName</key><string>PercyStar</string>
  <key>CFBundleIdentifier</key><string>dev.rpatty.percystar</string>
  <key>CFBundleExecutable</key><string>PercyStar</string>
  <key>CFBundleIconFile</key><string>PercyStar</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>${VERSION}</string>
  <key>CFBundleVersion</key><string>${VERSION}</string>
  <key>LSMinimumSystemVersion</key><string>${MACOSX_DEPLOYMENT_TARGET}</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSSupportsAutomaticGraphicsSwitching</key><true/>
</dict>
</plist>
PLIST

codesign --force --deep --sign - "$APP"
echo "Built $APP ($(du -sh "$APP" | cut -f1))"
lipo -info "$APP/Contents/MacOS/PercyStar"
