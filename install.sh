#!/bin/sh
# Tarn installer — curl -fsSL https://raw.githubusercontent.com/NazarKalytiuk/tarn/main/install.sh | sh
set -e

REPO="NazarKalytiuk/tarn"
INSTALL_DIR="${HIVE_INSTALL_DIR:-/usr/local/bin}"

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

ARTIFACT="tarn-${OS_TAG}-${ARCH_TAG}"

# Get latest release tag
echo "Fetching latest release..."
TAG=$(curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" | grep '"tag_name"' | sed 's/.*"tag_name": *"\([^"]*\)".*/\1/')

if [ -z "$TAG" ]; then
  echo "Error: Could not determine latest release"
  exit 1
fi

echo "Installing tarn ${TAG} (${OS_TAG}/${ARCH_TAG})..."

URL="https://github.com/${REPO}/releases/download/${TAG}/${ARTIFACT}.tar.gz"

# Download and extract
TMPDIR=$(mktemp -d)
trap 'rm -rf "$TMPDIR"' EXIT

curl -fsSL "$URL" -o "$TMPDIR/${ARTIFACT}.tar.gz"
tar xzf "$TMPDIR/${ARTIFACT}.tar.gz" -C "$TMPDIR"

# Install
if [ -w "$INSTALL_DIR" ]; then
  mv "$TMPDIR/$ARTIFACT" "$INSTALL_DIR/tarn"
else
  echo "Need sudo to install to $INSTALL_DIR"
  sudo mv "$TMPDIR/$ARTIFACT" "$INSTALL_DIR/tarn"
fi

chmod +x "$INSTALL_DIR/tarn"

echo ""
echo "  Tarn ${TAG} installed to ${INSTALL_DIR}/tarn"
echo ""
echo "  Run 'tarn --help' to get started"
echo ""
