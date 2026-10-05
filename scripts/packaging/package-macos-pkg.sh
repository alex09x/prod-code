#!/usr/bin/env bash
# package-macos-pkg.sh - Builds a macOS .pkg installer for prod-code using native pkgbuild.
set -euo pipefail

usage() {
    echo "Usage: $0 --version <version> --client-bin <path> --output <path.pkg>"
    exit 1
}

VERSION=""
CLIENT_BIN=""
OUTPUT=""

while [[ $# -gt 0 ]]; do
    case "$1" in
        --version) VERSION="$2"; shift 2 ;;
        --client-bin) CLIENT_BIN="$2"; shift 2 ;;
        --output) OUTPUT="$2"; shift 2 ;;
        *) usage ;;
    esac
done

if [[ -z "$VERSION" || -z "$CLIENT_BIN" || -z "$OUTPUT" ]]; then
    usage
fi

if [[ ! -f "$CLIENT_BIN" ]]; then
    echo "Error: client binary not found at $CLIENT_BIN" >&2
    exit 1
fi

TMP_DIR=$(mktemp -d)
trap 'rm -rf "$TMP_DIR"' EXIT

STAGING_DIR="$TMP_DIR/root"
mkdir -p "$STAGING_DIR"

# Copy binary to staging directory (installed into target install-location, e.g. /usr/local/bin)
cp "$CLIENT_BIN" "$STAGING_DIR/prod-code"
chmod 755 "$STAGING_DIR/prod-code"

# Code sign if on macOS
if command -v codesign >/dev/null 2>&1; then
    SIGN_IDENTITY="${PROD_CODE_SIGN_IDENTITY:-Developer ID Application: Alexander Panasenko (284V2M3LN9)}"
    codesign -s "$SIGN_IDENTITY" -f --options runtime --timestamp "$STAGING_DIR/prod-code" || codesign -s "$SIGN_IDENTITY" -f "$STAGING_DIR/prod-code" || codesign -s - -f "$STAGING_DIR/prod-code" || true
fi

# Build .pkg
mkdir -p "$(dirname "$OUTPUT")"
pkgbuild \
    --root "$STAGING_DIR" \
    --identifier "codes.prod.cli" \
    --version "$VERSION" \
    --install-location "/usr/local/bin" \
    "$OUTPUT"

echo "Successfully built macOS installer package: $OUTPUT ($(stat -f%z "$OUTPUT" 2>/dev/null || stat -c%s "$OUTPUT") bytes)"
