#!/usr/bin/env bash
# generate-homebrew-formula.sh - Generates a Homebrew formula for prod-code with release SHA-256 hashes.
set -euo pipefail

VERSION="${1:-0.3.19}"
TAG="v${VERSION#v}"
OUTPUT="${2:-Formula/prod-code.rb}"

REPO="alex09x/prod-code"
BASE_URL="https://github.com/${REPO}/releases/download/${TAG}"

echo "Generating Homebrew formula for ${TAG}..."

calc_sha() {
    local target="$1"
    local url="${BASE_URL}/prod-code-${target}"
    local sha
    echo "Fetching SHA256 for ${url}..." >&2
    sha=$(curl -sL "${url}" | shasum -a 256 | awk '{print $1}')
    echo "$sha"
}

SHA_MAC_ARM=$(calc_sha "aarch64-apple-darwin")
SHA_MAC_INTEL=$(calc_sha "x86_64-apple-darwin" || echo "0000000000000000000000000000000000000000000000000000000000000000")
SHA_LINUX_X86=$(calc_sha "x86_64-unknown-linux-gnu")
SHA_LINUX_ARM=$(calc_sha "aarch64-unknown-linux-gnu")

mkdir -p "$(dirname "$OUTPUT")"

cat << EOF > "$OUTPUT"
class ProdCode < Formula
  desc "Remote Code Intelligence for AI coding agents and editors (LSP, MCP)"
  homepage "https://prod.codes"
  version "${VERSION#v}"
  license any_of: ["MIT", "Apache-2.0"]

  on_macos do
    if Hardware::CPU.arm?
      url "${BASE_URL}/prod-code-aarch64-apple-darwin"
      sha256 "${SHA_MAC_ARM}"
    else
      url "${BASE_URL}/prod-code-x86_64-apple-darwin"
      sha256 "${SHA_MAC_INTEL}"
    end
  end

  on_linux do
    if Hardware::CPU.arm?
      url "${BASE_URL}/prod-code-aarch64-unknown-linux-gnu"
      sha256 "${SHA_LINUX_ARM}"
    else
      url "${BASE_URL}/prod-code-x86_64-unknown-linux-gnu"
      sha256 "${SHA_LINUX_X86}"
    end
  end

  def install
    cpu = Hardware::CPU.arm? ? "aarch64" : "x86_64"
    os = OS.mac? ? "apple-darwin" : "unknown-linux-gnu"
    bin.install "prod-code-#{cpu}-#{os}" => "prod-code"
  end

  test do
    assert_match "prod-code", shell_output("#{bin}/prod-code --help")
  end
end
EOF

echo "Successfully wrote Homebrew formula to $OUTPUT"
