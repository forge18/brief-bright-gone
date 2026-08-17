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

# --- download, verify, and extract -------------------------------------------
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

CHECKSUMS="SHA256SUMS"
ARCHIVE_BIN="bbg-${TARGET}"

echo "bbg: downloading ${ASSET} (v${VERSION}, ${TARGET})..."
curl -fsSL "${BASE_URL}/${ASSET}" -o "${TMP}/${ASSET}"
curl -fsSL "${BASE_URL}/${CHECKSUMS}" -o "${TMP}/${CHECKSUMS}"

EXPECTED_SHA="$(awk -v asset="$ASSET" '$2 == asset || $2 == "*" asset { print $1; exit }' "${TMP}/${CHECKSUMS}")"
[ -n "$EXPECTED_SHA" ] || { echo "bbg: checksum missing for ${ASSET}" >&2; exit 1; }
if command -v sha256sum >/dev/null 2>&1; then
  ACTUAL_SHA="$(sha256sum "${TMP}/${ASSET}" | awk '{print $1}')"
elif command -v shasum >/dev/null 2>&1; then
  ACTUAL_SHA="$(shasum -a 256 "${TMP}/${ASSET}" | awk '{print $1}')"
else
  echo "bbg: sha256sum or shasum is required for archive verification" >&2
  exit 1
fi
[ "$ACTUAL_SHA" = "$EXPECTED_SHA" ] || { echo "bbg: archive checksum mismatch" >&2; exit 1; }

ENTRIES="$(tar -tzf "${TMP}/${ASSET}")"
ENTRY_COUNT="$(printf '%s\n' "$ENTRIES" | awk 'NF { count += 1 } END { print count + 0 }')"
[ "$ENTRY_COUNT" -eq 1 ] && [ "$ENTRIES" = "$ARCHIVE_BIN" ] || {
  echo "bbg: archive must contain exactly the expected binary" >&2
  exit 1
}
tar -xzf "${TMP}/${ASSET}" -C "$TMP"

BIN_SRC="${TMP}/${ARCHIVE_BIN}"
[ -f "$BIN_SRC" ] || { echo "bbg: binary not found in archive" >&2; exit 1; }
install -m 0755 "$BIN_SRC" "$INSTALL_DIR/bbg"

# --- done --------------------------------------------------------------------
echo "bbg: installed to ${INSTALL_DIR}/bbg (v${VERSION})"
if ! echo ":$PATH:" | grep -q ":${INSTALL_DIR}:"; then
  echo "bbg: add to PATH:  export PATH=\"${INSTALL_DIR}:\$PATH\""
fi
echo "bbg: try:  echo 'please fix the bug' | bbg normalize"
