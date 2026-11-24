#!/usr/bin/env bash
# Build Intelli.app from target/release and wrap it in a drag-to-Applications .dmg.   Usage: build_dmg.sh VERSION ARCH  (arm64 | x86_64)
# Signing is optional: set CODESIGN_IDENTITY (a "Developer ID Application: ..." name) to sign; otherwise the app is ad-hoc signed,
# which runs but triggers Gatekeeper's "unidentified developer" prompt (right-click > Open the first time).
# NOT RUN YET: this script was written on Linux and has never been executed on a Mac.
set -euo pipefail
VERSION="${1:?version}"; ARCH="${2:?arch}"
BIN=target/release
for f in intelli-rust intelli-search; do [[ -x "$BIN/$f" ]] || { echo "missing $BIN/$f"; exit 1; }; done
APP="dist/Intelli.app"
rm -rf "$APP"; mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$BIN/intelli-search" "$APP/Contents/MacOS/IntelliSearch"   # what a double-click runs; it starts the sibling intelli-rust
cp "$BIN/intelli-rust" "$APP/Contents/MacOS/intelli-rust"
cp packaging/build/icon.icns "$APP/Contents/Resources/icon.icns"
cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleName</key><string>Intelli</string>
  <key>CFBundleDisplayName</key><string>Intelli Search</string>
  <key>CFBundleIdentifier</key><string>io.github.rahulparimal.intelli</string>
  <key>CFBundleVersion</key><string>${VERSION}</string>
  <key>CFBundleShortVersionString</key><string>${VERSION}</string>
  <key>CFBundleExecutable</key><string>IntelliSearch</string>
  <key>CFBundleIconFile</key><string>icon</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>NSHighResolutionCapable</key><true/>
</dict></plist>
PLIST
OUT="dist/Intelli-Rust-${VERSION}-macos-${ARCH}.dmg"
IDENTITY="${CODESIGN_IDENTITY:--}"
codesign --force --deep --options runtime --sign "$IDENTITY" "$APP"
codesign --verify --deep --strict "$APP"
STAGE="$(mktemp -d)"
cp -R "$APP" "$STAGE/"
ln -s /Applications "$STAGE/Applications"
cat > "$STAGE/READ ME FIRST.txt" <<TXT
Drag Intelli to Applications, then open it. It starts the search page in your browser.
If macOS says it cannot verify the developer: right-click Intelli, choose Open, then Open again (once).
Terminal users: /Applications/Intelli.app/Contents/MacOS/intelli-rust --help
TXT
rm -f "$OUT"
hdiutil create -volname "Intelli" -srcfolder "$STAGE" -ov -format UDZO "$OUT"
rm -rf "$STAGE"
echo "built $OUT"
