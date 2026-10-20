#!/bin/sh
# Installs the rlutter CLI from GitHub releases.
#
#   curl -fsSL https://raw.githubusercontent.com/nrbnlulu/flutter/rust/flutter-rs-shell/rust/install.sh | sh
#
# Environment:
#   RLUTTER_VERSION      release tag to install (default: rlutter, the rolling build)
#   RLUTTER_INSTALL_DIR  destination directory (default: $HOME/.local/bin)
#   RLUTTER_REPO         GitHub owner/repo (default: nrbnlulu/flutter)
set -eu

REPO="${RLUTTER_REPO:-nrbnlulu/flutter}"
VERSION="${RLUTTER_VERSION:-rlutter}"
INSTALL_DIR="${RLUTTER_INSTALL_DIR:-$HOME/.local/bin}"

say() { printf 'rlutter-install: %s\n' "$*"; }
die() { printf 'rlutter-install: error: %s\n' "$*" >&2; exit 1; }

os="$(uname -s)"
arch="$(uname -m)"
case "$os/$arch" in
  Linux/x86_64|Linux/amd64) asset="rlutter-linux-x64" ;;
  *) die "no prebuilt rlutter for $os/$arch (available: Linux x86_64)" ;;
esac

if command -v curl >/dev/null 2>&1; then
  fetch() { curl -fsSL -o "$2" "$1"; }
elif command -v wget >/dev/null 2>&1; then
  fetch() { wget -q -O "$2" "$1"; }
else
  die "curl or wget is required"
fi

if command -v sha256sum >/dev/null 2>&1; then
  sha256() { sha256sum "$1" | cut -d' ' -f1; }
elif command -v shasum >/dev/null 2>&1; then
  sha256() { shasum -a 256 "$1" | cut -d' ' -f1; }
else
  die "sha256sum or shasum is required"
fi

base="https://github.com/$REPO/releases/download/$VERSION"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT INT TERM

say "downloading $asset ($VERSION) from $REPO"
fetch "$base/$asset.tar.gz" "$tmp/$asset.tar.gz" || die "download failed: $base/$asset.tar.gz"
fetch "$base/$asset.tar.gz.sha256" "$tmp/$asset.tar.gz.sha256" || die "checksum download failed"

expected="$(cut -d' ' -f1 "$tmp/$asset.tar.gz.sha256")"
actual="$(sha256 "$tmp/$asset.tar.gz")"
[ "$expected" = "$actual" ] || die "checksum mismatch (expected $expected, got $actual)"

tar -xzf "$tmp/$asset.tar.gz" -C "$tmp"
mkdir -p "$INSTALL_DIR"
mv "$tmp/$asset" "$INSTALL_DIR/rlutter"
chmod +x "$INSTALL_DIR/rlutter"

say "installed $INSTALL_DIR/rlutter"
case ":$PATH:" in
  *":$INSTALL_DIR:"*) ;;
  *) say "add it to your PATH:  export PATH=\"$INSTALL_DIR:\$PATH\"" ;;
esac
