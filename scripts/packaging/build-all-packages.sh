#!/usr/bin/env bash
# build-all-packages.sh - Builds Debian packages, macOS installer .pkg, macOS .dmg, Homebrew formula, and SHA256 checksums.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"

VERSION="${1:-0.3.19}"
OUT_DIR="${ROOT_DIR}/dist/packages"
mkdir -p "$OUT_DIR"

echo "Building release packages for prod-code v${VERSION}..."

CLIENT_BIN="${CLIENT_BIN:-$HOME/.cargo/bin/prod-code}"
if [[ ! -f "$CLIENT_BIN" ]]; then
    echo "Warning: $CLIENT_BIN not found, attempting to find in PATH..."
    CLIENT_BIN="$(which prod-code || true)"
fi

if [[ -z "$CLIENT_BIN" || ! -f "$CLIENT_BIN" ]]; then
    echo "Error: prod-code binary not found." >&2
    exit 1
fi

echo "Using prod-code binary: $CLIENT_BIN"

# 1. Debian packages (.deb)
echo "Generating Debian package (arm64)..."
python3 "$SCRIPT_DIR/package-deb.py" \
    --version "$VERSION" \
    --arch "arm64" \
    --client-bin "$CLIENT_BIN" \
    --service-file "$ROOT_DIR/crates/prod-code-gateway/prod-code-gateway.service" \
    --readme "$ROOT_DIR/README.md" \
    --output "$OUT_DIR/prod-code_${VERSION}_arm64.deb"

echo "Generating Debian package (amd64)..."
python3 "$SCRIPT_DIR/package-deb.py" \
    --version "$VERSION" \
    --arch "amd64" \
    --client-bin "$CLIENT_BIN" \
    --service-file "$ROOT_DIR/crates/prod-code-gateway/prod-code-gateway.service" \
    --readme "$ROOT_DIR/README.md" \
    --output "$OUT_DIR/prod-code_${VERSION}_amd64.deb"

# 2. macOS .pkg & .dmg (if on macOS)
if [[ "$(uname -s)" == "Darwin" ]]; then
    echo "Generating macOS installer package (.pkg)..."
    "$SCRIPT_DIR/package-macos-pkg.sh" \
        --version "$VERSION" \
        --client-bin "$CLIENT_BIN" \
        --output "$OUT_DIR/prod-code-${VERSION}-macOS.pkg"

    echo "Generating macOS disk image (.dmg)..."
    "$SCRIPT_DIR/package-macos-dmg.sh" \
        --version "$VERSION" \
        --client-bin "$CLIENT_BIN" \
        --output "$OUT_DIR/prod-code-${VERSION}-macOS.dmg"
fi

# 3. Copy universal installer
cp "$SCRIPT_DIR/install.sh" "$OUT_DIR/install.sh"

# 4. Generate SHA256 checksums
echo "Generating SHA256SUMS..."
cd "$OUT_DIR"
shasum -a 256 prod-code* > SHA256SUMS

echo ""
echo "=== Packaging Complete ==="
ls -lh "$OUT_DIR"
