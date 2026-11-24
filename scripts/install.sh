#!/usr/bin/env sh
# One-line installer for macOS and Linux (no root needed):
#   curl -LsSf https://raw.githubusercontent.com/rahulparimal/Intelligent-Browser-fast/main/scripts/install.sh | sh
# Downloads the release tarball for this computer, checks its SHA-256, and installs to ~/.local (override with INTELLI_PREFIX).
# Overrides: INTELLI_VERSION (default: latest release), INTELLI_REPO, INTELLI_RELEASE_URL (a folder or URL holding the tarball and
# its SHA256SUMS file; used for testing and mirrors). Safe to re-run: it upgrades in place.
set -eu

REPO="${INTELLI_REPO:-rahulparimal/Intelligent-Browser-fast}"
PREFIX="${INTELLI_PREFIX:-$HOME/.local}"

case "$(uname -s)" in
	Linux) os=linux ;;
	Darwin) os=macos ;;
	*) echo "This installer supports macOS and Linux. On Windows use scripts/install.ps1." >&2; exit 1 ;;
esac
case "$(uname -m)" in
	x86_64|amd64) arch=x64; [ "$os" = macos ] && arch=x86_64 ;;
	arm64|aarch64) arch=arm64 ;;
	*) echo "Unsupported processor: $(uname -m)" >&2; exit 1 ;;
esac
platform="$os-$arch"

version="${INTELLI_VERSION:-}"
if [ -z "$version" ] && [ -z "${INTELLI_RELEASE_URL:-}" ]; then
	tag="$(curl -fsSI -o /dev/null -w '%{redirect_url}' "https://github.com/$REPO/releases/latest" | sed 's|.*/tag/||')"
	[ -n "$tag" ] || { echo "Could not find the latest release of $REPO (has one been published?)." >&2; exit 1; }
	version="${tag#v}"
fi
version="${version#v}"
base="${INTELLI_RELEASE_URL:-https://github.com/$REPO/releases/download/v$version}"

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
file="intelli-rust-$version-$platform.tar.gz"
echo "==> Downloading $file"
curl -fL --proto '=https,file' -o "$tmp/$file" "$base/$file"
curl -fL --proto '=https,file' -o "$tmp/SHA256SUMS" "$base/SHA256SUMS-$platform.txt"

echo "==> Checking the download"
expected="$(grep " $file\$" "$tmp/SHA256SUMS" | awk '{print $1}')"
[ -n "$expected" ] || { echo "No checksum listed for $file." >&2; exit 1; }
if command -v sha256sum >/dev/null 2>&1; then actual="$(sha256sum "$tmp/$file" | awk '{print $1}')"; else actual="$(shasum -a 256 "$tmp/$file" | awk '{print $1}')"; fi
[ "$expected" = "$actual" ] || { echo "Checksum mismatch: refusing to install." >&2; exit 1; }

echo "==> Installing to $PREFIX"
tar -xzf "$tmp/$file" -C "$tmp"
PREFIX="$PREFIX" sh "$tmp/intelli-rust/install.sh"
case ":$PATH:" in *":$PREFIX/bin:"*) ;; *) echo "Add $PREFIX/bin to your PATH to use: intelli-rust" ;; esac
echo "Try: intelli-rust index ~/Documents && intelli-rust search \"your question\""
