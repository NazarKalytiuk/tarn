#!/usr/bin/env bash
set -euo pipefail

if [ "$#" -ne 1 ]; then
  echo "usage: $0 <output-path>" >&2
  exit 1
fi

: "${TAG:?TAG is required}"
: "${LINUX_AMD64_SHA:?LINUX_AMD64_SHA is required}"
: "${LINUX_ARM64_SHA:?LINUX_ARM64_SHA is required}"
: "${DARWIN_AMD64_SHA:?DARWIN_AMD64_SHA is required}"
: "${DARWIN_ARM64_SHA:?DARWIN_ARM64_SHA is required}"

OUT="$1"

cat >"$OUT" <<EOF
class Tarn < Formula
  desc "CLI-first API testing tool"
  homepage "https://github.com/NazarKalytiuk/hive"
  version "${TAG#v}"
  license "MIT"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/NazarKalytiuk/hive/releases/download/${TAG}/hive-darwin-arm64.tar.gz"
      sha256 "${DARWIN_ARM64_SHA}"
    else
      url "https://github.com/NazarKalytiuk/hive/releases/download/${TAG}/hive-darwin-amd64.tar.gz"
      sha256 "${DARWIN_AMD64_SHA}"
    end
  end

  on_linux do
    if Hardware::CPU.arm?
      url "https://github.com/NazarKalytiuk/hive/releases/download/${TAG}/hive-linux-arm64.tar.gz"
      sha256 "${LINUX_ARM64_SHA}"
    else
      url "https://github.com/NazarKalytiuk/hive/releases/download/${TAG}/hive-linux-amd64.tar.gz"
      sha256 "${LINUX_AMD64_SHA}"
    end
  end

  def install
    bin.install "tarn"
    bin.install "tarn-mcp"
    bin.install "tarn-lsp"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/tarn --version")
  end
end
EOF
