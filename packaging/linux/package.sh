#!/usr/bin/env bash
# Build the Linux (or any Unix) tarball from target/release.  Usage: packaging/linux/package.sh VERSION PLATFORM   e.g. 0.1.0 linux-x64
# Output: dist/intelli-rust-VERSION-PLATFORM.tar.gz containing bin/, share/ (desktop entry, icon) and an install.sh that works
# without root (installs under ~/.local).
set -euo pipefail
VERSION="${1:?version}"; PLATFORM="${2:?platform, e.g. linux-x64}"
BIN=target/release
for f in intelli-rust intelli-search; do [[ -x "$BIN/$f" ]] || { echo "missing $BIN/$f (run: cargo build --release -p ibos-cli -p ibos-launcher)"; exit 1; }; done
STAGE="$(mktemp -d)/intelli-rust"
mkdir -p "$STAGE/bin" "$STAGE/share/applications" "$STAGE/share/icons/hicolor/256x256/apps"
cp "$BIN/intelli-rust" "$BIN/intelli-search" "$STAGE/bin/"
cp packaging/build/icon.png "$STAGE/share/icons/hicolor/256x256/apps/intelli-search.png"
cat > "$STAGE/share/applications/intelli-search.desktop" <<'DESKTOP'
[Desktop Entry]
Type=Application
Name=Intelli Search
Comment=Search your documents on your own computer
Exec=intelli-search
Icon=intelli-search
Terminal=false
Categories=Utility;Office;
DESKTOP
cp LICENSE "$STAGE/" 2>/dev/null || true
cat > "$STAGE/install.sh" <<'INSTALL'
#!/usr/bin/env sh
# Install from this folder without root: copies the programs to $PREFIX/bin (default ~/.local) and adds the menu entry.
set -eu
PREFIX="${PREFIX:-$HOME/.local}"
HERE="$(cd "$(dirname "$0")" && pwd)"
mkdir -p "$PREFIX/bin" "$PREFIX/share/applications" "$PREFIX/share/icons/hicolor/256x256/apps"
cp "$HERE/bin/intelli-rust" "$HERE/bin/intelli-search" "$PREFIX/bin/"
cp "$HERE/share/icons/hicolor/256x256/apps/intelli-search.png" "$PREFIX/share/icons/hicolor/256x256/apps/"
cp "$HERE/share/applications/intelli-search.desktop" "$PREFIX/share/applications/"
echo "Installed to $PREFIX. Make sure $PREFIX/bin is on your PATH, then run: intelli-rust --help"
INSTALL
chmod +x "$STAGE/install.sh" "$STAGE/bin/"*
mkdir -p dist
OUT="dist/intelli-rust-${VERSION}-${PLATFORM}.tar.gz"
tar -C "$(dirname "$STAGE")" -czf "$OUT" intelli-rust
rm -rf "$(dirname "$STAGE")"
echo "built $OUT"
