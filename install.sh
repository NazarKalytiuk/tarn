#!/bin/sh
# Tarn installer — curl -fsSL https://raw.githubusercontent.com/NazarKalytiuk/hive/main/install.sh | sh
set -e

REPO="NazarKalytiuk/hive"
INSTALL_DIR="${TARN_INSTALL_DIR:-${HIVE_INSTALL_DIR:-/usr/local/bin}}"

# Detect OS and architecture
OS="$(uname -s)"
ARCH="$(uname -m)"

case "$OS" in
  Linux)  OS_TAG="linux" ;;
  Darwin) OS_TAG="darwin" ;;
  *)      echo "Unsupported OS: $OS"; exit 1 ;;
esac

case "$ARCH" in
  x86_64|amd64)  ARCH_TAG="amd64" ;;
  aarch64|arm64) ARCH_TAG="arm64" ;;
  *)             echo "Unsupported architecture: $ARCH"; exit 1 ;;
esac

ARTIFACT="hive-${OS_TAG}-${ARCH_TAG}"

# Get latest release tag
echo "Fetching latest release..."
TAG=$(curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" | grep '"tag_name"' | sed 's/.*"tag_name": *"\([^"]*\)".*/\1/')

if [ -z "$TAG" ]; then
  echo "Error: Could not determine latest release"
  exit 1
fi

echo "Installing tarn ${TAG} (${OS_TAG}/${ARCH_TAG})..."

URL="https://github.com/${REPO}/releases/download/${TAG}/${ARTIFACT}.tar.gz"
CHECKSUM_URL="https://github.com/${REPO}/releases/download/${TAG}/hive-checksums.txt"

# Download and extract
TMPDIR=$(mktemp -d)
trap 'rm -rf "$TMPDIR"' EXIT

curl -fsSL "$URL" -o "$TMPDIR/${ARTIFACT}.tar.gz"
curl -fsSL "$CHECKSUM_URL" -o "$TMPDIR/hive-checksums.txt"

EXPECTED_SHA="$(grep " ${ARTIFACT}.tar.gz$" "$TMPDIR/hive-checksums.txt" | awk '{print $1}')"
if [ -z "$EXPECTED_SHA" ]; then
  echo "Error: Checksum not found for ${ARTIFACT}.tar.gz"
  exit 1
fi

if command -v sha256sum >/dev/null 2>&1; then
  ACTUAL_SHA="$(sha256sum "$TMPDIR/${ARTIFACT}.tar.gz" | awk '{print $1}')"
elif command -v shasum >/dev/null 2>&1; then
  ACTUAL_SHA="$(shasum -a 256 "$TMPDIR/${ARTIFACT}.tar.gz" | awk '{print $1}')"
else
  echo "Error: Neither sha256sum nor shasum is available for checksum verification"
  exit 1
fi

if [ "$EXPECTED_SHA" != "$ACTUAL_SHA" ]; then
  echo "Error: Checksum verification failed for ${ARTIFACT}.tar.gz"
  exit 1
fi

tar xzf "$TMPDIR/${ARTIFACT}.tar.gz" -C "$TMPDIR"

# Install
if [ -w "$INSTALL_DIR" ]; then
  mv "$TMPDIR/tarn" "$INSTALL_DIR/tarn" 2>/dev/null || mv "$TMPDIR/$ARTIFACT" "$INSTALL_DIR/tarn"
else
  echo "Need sudo to install to $INSTALL_DIR"
  sudo mv "$TMPDIR/tarn" "$INSTALL_DIR/tarn" 2>/dev/null || sudo mv "$TMPDIR/$ARTIFACT" "$INSTALL_DIR/tarn"
fi

chmod +x "$INSTALL_DIR/tarn"

echo ""
echo "  Tarn ${TAG} installed to ${INSTALL_DIR}/tarn"
echo ""
echo "  Run 'tarn --help' to get started"
echo ""
