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
CHECKSUM_URL="https://github.com/${REPO}/releases/download/${TAG}/tarn-checksums.txt"

# Download and extract
TMPDIR=$(mktemp -d)
trap 'rm -rf "$TMPDIR"' EXIT

curl -fsSL "$URL" -o "$TMPDIR/${ARTIFACT}.tar.gz"

# Verify checksum if available
if curl -fsSL "$CHECKSUM_URL" -o "$TMPDIR/tarn-checksums.txt" 2>/dev/null; then
  EXPECTED_SHA="$(grep " ${ARTIFACT}.tar.gz$" "$TMPDIR/tarn-checksums.txt" | awk '{print $1}')"
  if [ -n "$EXPECTED_SHA" ]; then
    if command -v sha256sum >/dev/null 2>&1; then
      ACTUAL_SHA="$(sha256sum "$TMPDIR/${ARTIFACT}.tar.gz" | awk '{print $1}')"
    elif command -v shasum >/dev/null 2>&1; then
      ACTUAL_SHA="$(shasum -a 256 "$TMPDIR/${ARTIFACT}.tar.gz" | awk '{print $1}')"
    else
      ACTUAL_SHA=""
    fi

    if [ -n "$ACTUAL_SHA" ] && [ "$EXPECTED_SHA" != "$ACTUAL_SHA" ]; then
      echo "Error: Checksum verification failed for ${ARTIFACT}.tar.gz"
      exit 1
    fi
    echo "Checksum verified."
  fi
else
  echo "Checksums not available for this release, skipping verification."
fi

tar xzf "$TMPDIR/${ARTIFACT}.tar.gz" -C "$TMPDIR"

# Install
for BIN in tarn tarn-mcp tarn-lsp; do
  if [ -f "$TMPDIR/$BIN" ]; then
    if [ -w "$INSTALL_DIR" ]; then
      mv "$TMPDIR/$BIN" "$INSTALL_DIR/$BIN"
    else
      echo "Need sudo to install to $INSTALL_DIR"
      sudo mv "$TMPDIR/$BIN" "$INSTALL_DIR/$BIN"
    fi
    chmod +x "$INSTALL_DIR/$BIN"
  fi
done

# Fallback for older releases that used the artifact name as the binary
if [ ! -f "$INSTALL_DIR/tarn" ] && [ -f "$TMPDIR/$ARTIFACT" ]; then
  if [ -w "$INSTALL_DIR" ]; then
    mv "$TMPDIR/$ARTIFACT" "$INSTALL_DIR/tarn"
  else
    sudo mv "$TMPDIR/$ARTIFACT" "$INSTALL_DIR/tarn"
  fi
  chmod +x "$INSTALL_DIR/tarn"
fi

echo ""
echo "  Tarn ${TAG} installed to ${INSTALL_DIR}/tarn"
if [ -f "$INSTALL_DIR/tarn-mcp" ]; then
  echo "  Tarn MCP ${TAG} installed to ${INSTALL_DIR}/tarn-mcp"
fi
if [ -f "$INSTALL_DIR/tarn-lsp" ]; then
  echo "  Tarn LSP ${TAG} installed to ${INSTALL_DIR}/tarn-lsp"
fi
echo ""
echo "  Run 'tarn --help' to get started"
echo ""
