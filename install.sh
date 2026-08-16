#!/usr/bin/env bash
# bbg — brief, bright, gone. Installer shim.
#
# One-line install:
#   curl -fsSL https://raw.githubusercontent.com/forge18/brief-bright-gone/main/install.sh | bash
#
# Downloads the prebuilt binary for your OS/arch from the latest GitHub
# Release and installs to ~/.local/bin (or $BBG_INSTALL_DIR / /usr/local/bin
# when set / writable).
#
# Requires: curl, tar. Rust toolchain NOT required (prebuilt binaries).

set -euo pipefail

REPO="forge18/brief-bright-gone"
VERSION="${BBG_VERSION:-latest}"
INSTALL_DIR="${BBG_INSTALL_DIR:-}"

# --- platform detection ------------------------------------------------------
OS="$(uname -s)"
ARCH="$(uname -m)"

case "$OS" in
  Darwin)  OS_NAME="apple-darwin" ;;
  Linux)   OS_NAME="unknown-linux-gnu" ;;
  *)       echo "bbg: unsupported OS '$OS' (install from source: cargo install brief-bright-gone)" >&2; exit 1 ;;
esac

case "$ARCH" in
  arm64|aarch64) ARCH_NAME="aarch64" ;;
  x86_64|amd64)  ARCH_NAME="x86_64" ;;
  *)             echo "bbg: unsupported arch '$ARCH'" >&2; exit 1 ;;
esac

TARGET="${ARCH_NAME}-${OS_NAME}"
ASSET="bbg-${TARGET}.tar.gz"

# --- resolve version ---------------------------------------------------------
if [ "$VERSION" = "latest" ]; then
  VERSION="$(curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" | grep '"tag_name"' | head -1 | sed -E 's/.*"v?([^"]+)".*/\1/')"
fi

BASE_URL="https://github.com/${REPO}/releases/download/v${VERSION}"

# --- install dir -------------------------------------------------------------
if [ -z "$INSTALL_DIR" ]; then
  if [ -w "/usr/local/bin" ]; then
    INSTALL_DIR="/usr/local/bin"
  else
    INSTALL_DIR="$HOME/.local/bin"
  fi
fi
mkdir -p "$INSTALL_DIR"

# --- download + extract ------------------------------------------------------
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

echo "bbg: downloading ${ASSET} (v${VERSION}, ${TARGET})..."
curl -fsSL "${BASE_URL}/${ASSET}" -o "${TMP}/${ASSET}"
tar -xzf "${TMP}/${ASSET}" -C "$TMP"

# The tarball contains the binary named after the target; install as `bbg`.
BIN_SRC="$(find "$TMP" -type f -name 'bbg-*' ! -name '*.tar.gz' | head -1)"
[ -n "$BIN_SRC" ] || { echo "bbg: binary not found in archive" >&2; exit 1; }

install -m 0755 "$BIN_SRC" "$INSTALL_DIR/bbg"

# --- done --------------------------------------------------------------------
echo "bbg: installed to ${INSTALL_DIR}/bbg (v${VERSION})"
if ! echo ":$PATH:" | grep -q ":${INSTALL_DIR}:"; then
  echo "bbg: add to PATH:  export PATH=\"${INSTALL_DIR}:\$PATH\""
fi
echo "bbg: try:  echo 'please fix the bug' | bbg normalize"
