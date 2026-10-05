#!/usr/bin/env bash
# build-all-packages.sh - Builds Debian packages, RPM packages, Arch packages, macOS installer .pkg, macOS .dmg, Homebrew formula, and SHA256 checksums.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"

CARGO_TOML_VERSION="$(grep '^version = ' "$ROOT_DIR/Cargo.toml" | head -1 | cut -d'"' -f2)"
VERSION="${1:-$CARGO_TOML_VERSION}"
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

if [[ -z "${CLIENT_BIN_X86_64:-}" ]]; then
    for candidate in \
        "$ROOT_DIR/target/x86_64-unknown-linux-gnu/release/prod-code" \
        "$ROOT_DIR/dist/bin/prod-code-x86_64-unknown-linux-gnu"; do
        if [[ -f "$candidate" ]]; then
            CLIENT_BIN_X86_64="$candidate"
            break
        fi
    done
fi

if [[ -z "${CLIENT_BIN_AARCH64:-}" ]]; then
    for candidate in \
        "$ROOT_DIR/target/aarch64-unknown-linux-gnu/release/prod-code" \
        "$ROOT_DIR/dist/bin/prod-code-aarch64-unknown-linux-gnu"; do
        if [[ -f "$candidate" ]]; then
            CLIENT_BIN_AARCH64="$candidate"
            break
        fi
    done
fi

CLIENT_BIN_X86_64="${CLIENT_BIN_X86_64:-}"
CLIENT_BIN_AARCH64="${CLIENT_BIN_AARCH64:-}"
if [[ -z "$CLIENT_BIN_X86_64" || ! -f "$CLIENT_BIN_X86_64" || -z "$CLIENT_BIN_AARCH64" || ! -f "$CLIENT_BIN_AARCH64" ]]; then
    echo "Error: Linux binaries not found in target directories or environment variables." >&2
    echo "Set CLIENT_BIN_X86_64 and CLIENT_BIN_AARCH64 to the compiled Linux binaries, or build them with:" >&2
    echo "  cargo build --release --target x86_64-unknown-linux-gnu" >&2
    echo "  cargo build --release --target aarch64-unknown-linux-gnu" >&2
    echo "See docs/distribution.md for the complete release workflow." >&2
    exit 1
fi

verify_linux_arch() {
    python3 - "$1" "$2" <<'PYCODE'
import pathlib, sys
path, expected = sys.argv[1:]
with open(path, "rb") as executable:
    data = executable.read(20)
if len(data) < 20 or data[:4] != b"\x7fELF" or data[5] not in (1, 2):
    raise SystemExit(f"Error: {path} is not a valid ELF binary for {expected}")
actual_id = int.from_bytes(data[18:20], "little" if data[5] == 1 else "big")
actual = {62: "x86_64", 183: "aarch64"}.get(actual_id, "unknown")
if actual != expected:
    raise SystemExit(f"Error: {path} is {actual}, expected {expected}")
PYCODE
}
verify_linux_arch "$CLIENT_BIN_X86_64" x86_64
verify_linux_arch "$CLIENT_BIN_AARCH64" aarch64

# 1. Debian packages (.deb)
echo "Generating Debian package (arm64)..."
python3 "$SCRIPT_DIR/package-deb.py" \
    --version "$VERSION" \
    --arch "arm64" \
    --client-bin "$CLIENT_BIN_AARCH64" \
    --service-file "$ROOT_DIR/crates/prod-code-gateway/prod-code-gateway.service" \
    --readme "$ROOT_DIR/README.md" \
    --output "$OUT_DIR/prod-code_${VERSION}_arm64.deb"

echo "Generating Debian package (amd64)..."
python3 "$SCRIPT_DIR/package-deb.py" \
    --version "$VERSION" \
    --arch "amd64" \
    --client-bin "$CLIENT_BIN_X86_64" \
    --service-file "$ROOT_DIR/crates/prod-code-gateway/prod-code-gateway.service" \
    --readme "$ROOT_DIR/README.md" \
    --output "$OUT_DIR/prod-code_${VERSION}_amd64.deb"

# 2. RPM packages (.rpm)
echo "Generating RPM package (aarch64)..."
python3 "$SCRIPT_DIR/package-rpm.py" \
    --version "$VERSION" \
    --arch "aarch64" \
    --client-bin "$CLIENT_BIN_AARCH64" \
    --service-file "$ROOT_DIR/crates/prod-code-gateway/prod-code-gateway.service" \
    --readme "$ROOT_DIR/README.md" \
    --output "$OUT_DIR/prod-code-${VERSION}-1.aarch64.rpm"

echo "Generating RPM package (x86_64)..."
python3 "$SCRIPT_DIR/package-rpm.py" \
    --version "$VERSION" \
    --arch "x86_64" \
    --client-bin "$CLIENT_BIN_X86_64" \
    --service-file "$ROOT_DIR/crates/prod-code-gateway/prod-code-gateway.service" \
    --readme "$ROOT_DIR/README.md" \
    --output "$OUT_DIR/prod-code-${VERSION}-1.x86_64.rpm"

# 3. Arch Linux packages (.pkg.tar.gz & PKGBUILD)
echo "Generating Arch Linux package (x86_64)..."
"$SCRIPT_DIR/package-arch.sh" \
    --version "$VERSION" \
    --arch "x86_64" \
    --client-bin "$CLIENT_BIN_X86_64" \
    --service-file "$ROOT_DIR/crates/prod-code-gateway/prod-code-gateway.service" \
    --readme "$ROOT_DIR/README.md" \
    --output-dir "$OUT_DIR"

echo "Generating Arch Linux package (aarch64)..."
"$SCRIPT_DIR/package-arch.sh" \
    --version "$VERSION" \
    --arch "aarch64" \
    --client-bin "$CLIENT_BIN_AARCH64" \
    --service-file "$ROOT_DIR/crates/prod-code-gateway/prod-code-gateway.service" \
    --readme "$ROOT_DIR/README.md" \
    --output-dir "$OUT_DIR"

# 4. macOS .pkg & .dmg (if on macOS)
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

# 5. Copy universal installer
cp "$SCRIPT_DIR/install.sh" "$OUT_DIR/install.sh"

# 6. Generate SHA256 checksums
echo "Generating SHA256SUMS..."
cd "$OUT_DIR"
shasum -a 256 prod-code* > SHA256SUMS

echo ""
echo "=== Packaging Complete ==="
ls -lh "$OUT_DIR"
