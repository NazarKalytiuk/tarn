#!/usr/bin/env bash
set -euo pipefail

REPO="NazarKalytiuk/tarn"
VERSION="${TARN_VERSION:-latest}"

# Detect OS
case "$(uname -s)" in
  Linux*)  OS="linux" ;;
  Darwin*) OS="darwin" ;;
  *)
    echo "Error: Unsupported operating system: $(uname -s)"
    exit 1
    ;;
esac

# Detect architecture
case "$(uname -m)" in
  x86_64)  ARCH="amd64" ;;
  aarch64) ARCH="arm64" ;;
  arm64)   ARCH="arm64" ;;
  *)
    echo "Error: Unsupported architecture: $(uname -m)"
    exit 1
    ;;
esac

echo "Detected platform: ${OS}/${ARCH}"

# Resolve latest version if needed
if [ "${VERSION}" = "latest" ]; then
  echo "Fetching latest release tag..."
  VERSION=$(curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" | grep '"tag_name"' | sed -E 's/.*"tag_name": *"([^"]+)".*/\1/')
  if [ -z "${VERSION}" ]; then
    echo "Error: Failed to determine latest release version"
    exit 1
  fi
  echo "Latest version: ${VERSION}"
fi

# Build download URL
ARCHIVE="tarn-${OS}-${ARCH}.tar.gz"
DOWNLOAD_URL="https://github.com/${REPO}/releases/download/${VERSION}/${ARCHIVE}"

echo "Downloading ${DOWNLOAD_URL}..."
TMPDIR=$(mktemp -d)
trap 'rm -rf "${TMPDIR}"' EXIT

curl -fsSL "${DOWNLOAD_URL}" -o "${TMPDIR}/${ARCHIVE}"

echo "Extracting to /usr/local/bin/tarn..."
tar -xzf "${TMPDIR}/${ARCHIVE}" -C "${TMPDIR}"
sudo install -m 755 "${TMPDIR}/tarn" /usr/local/bin/tarn

echo "Installed tarn version:"
tarn --version
