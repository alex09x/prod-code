#!/usr/bin/env bash
# package-arch.sh - Generates Arch Linux PKGBUILD and native pacman binary packages (.pkg.tar.gz)
# Pure POSIX shell / python standard library implementation: runs on macOS and Linux without makepkg.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"

VERSION=""
ARCH="x86_64"
CLIENT_BIN=""
SERVICE_FILE="${ROOT_DIR}/crates/prod-code-gateway/prod-code-gateway.service"
README_FILE="${ROOT_DIR}/README.md"
OUTPUT_DIR="${ROOT_DIR}/dist/packages"
PKGBUILD_OUT=""

while [[ $# -gt 0 ]]; do
    case "$1" in
        --version)
            VERSION="$2"
            shift 2
            ;;
        --arch)
            ARCH="$2"
            shift 2
            ;;
        --client-bin)
            CLIENT_BIN="$2"
            shift 2
            ;;
        --service-file)
            SERVICE_FILE="$2"
            shift 2
            ;;
        --readme)
            README_FILE="$2"
            shift 2
            ;;
        --output-dir)
            OUTPUT_DIR="$2"
            shift 2
            ;;
        --pkgbuild-out)
            PKGBUILD_OUT="$2"
            shift 2
            ;;
        *)
            echo "Unknown argument: $1" >&2
            exit 1
            ;;
    esac
done

if [[ -z "$VERSION" ]]; then
    VERSION="$(grep '^version = ' "$ROOT_DIR/Cargo.toml" | head -1 | cut -d'"' -f2)"
fi
VERSION="${VERSION#v}"

# Normalize architecture to Arch Linux convention (x86_64 or aarch64)
case "$ARCH" in
    x86_64|amd64)
        ARCH="x86_64"
        ;;
    aarch64|arm64)
        ARCH="aarch64"
        ;;
    *)
        echo "Unsupported architecture for Arch Linux: $ARCH" >&2
        exit 1
        ;;
esac

mkdir -p "$OUTPUT_DIR"
if [[ ! -f "$SERVICE_FILE" || ! -f "$README_FILE" ]]; then
    echo "Error: service file and README are required to generate a usable PKGBUILD." >&2
    exit 1
fi
SERVICE_SHA256="$(shasum -a 256 "$SERVICE_FILE" | cut -d' ' -f1)"
README_SHA256="$(shasum -a 256 "$README_FILE" | cut -d' ' -f1)"

# 1. Generate PKGBUILD for AUR / makepkg
PKGBUILD_CONTENT="# Maintainer: Alexander Panasenko <alex@prod.codes>
pkgname=prod-code
pkgver=${VERSION}
pkgrel=1
pkgdesc=\"Remote Code Intelligence for AI coding agents and editors\"
arch=('x86_64' 'aarch64')
url=\"https://prod.codes\"
license=('Apache-2.0')
depends=()
optdepends=('systemd: for background user service')
source=(\"prod-code-gateway.service\" \"README.md\")
source_x86_64=(\"https://github.com/alex09x/prod-code/releases/download/v\${pkgver}/prod-code-x86_64-unknown-linux-gnu\")
source_aarch64=(\"https://github.com/alex09x/prod-code/releases/download/v\${pkgver}/prod-code-aarch64-unknown-linux-gnu\")
sha256sums=(\"${SERVICE_SHA256}\" \"${README_SHA256}\")
sha256sums_x86_64=('SKIP')
sha256sums_aarch64=('SKIP')

package() {
    install -Dm755 \"\${srcdir}/prod-code-\${CARCH}-unknown-linux-gnu\" \"\${pkgdir}/usr/bin/prod-code\"
    install -Dm644 \"\${srcdir}/prod-code-gateway.service\" \"\${pkgdir}/usr/lib/systemd/user/prod-code-gateway.service\"
    install -Dm644 \"\${srcdir}/README.md\" \"\${pkgdir}/usr/share/doc/prod-code/README.md\"
}
"

if [[ -n "$PKGBUILD_OUT" ]]; then
    PKGBUILD_PATH="$PKGBUILD_OUT"
    PKGBUILD_DIR="$(dirname "$PKGBUILD_OUT")"
else
    PKGBUILD_PATH="$OUTPUT_DIR/PKGBUILD"
    PKGBUILD_DIR="$OUTPUT_DIR"
fi
mkdir -p "$PKGBUILD_DIR"
printf '%s\n' "$PKGBUILD_CONTENT" > "$PKGBUILD_PATH"
cp "$SERVICE_FILE" "$PKGBUILD_DIR/prod-code-gateway.service"
cp "$README_FILE" "$PKGBUILD_DIR/README.md"
echo "✓ PKGBUILD generated: $PKGBUILD_PATH"

# 2. If client binary is provided, generate standalone .pkg.tar.gz package
if [[ -n "$CLIENT_BIN" && -f "$CLIENT_BIN" ]]; then
    PKG_TAR="${OUTPUT_DIR}/prod-code-${VERSION}-1-${ARCH}.pkg.tar.gz"
    echo "Building Arch Linux binary package: $PKG_TAR..."

    python3 - << PYEOF
import os
import tarfile
import time
import io

version = "${VERSION}"
arch = "${ARCH}"
client_bin = "${CLIENT_BIN}"
service_file = "${SERVICE_FILE}"
readme_file = "${README_FILE}"
out_path = "${PKG_TAR}"

now = int(time.time())
bin_size = os.path.getsize(client_bin)
svc_size = os.path.getsize(service_file) if os.path.exists(service_file) else 0
readme_size = os.path.getsize(readme_file) if os.path.exists(readme_file) else 0
installed_size = bin_size + svc_size + readme_size

pkginfo = f"""pkgname = prod-code
pkgbase = prod-code
pkgver = {version}-1
pkgdesc = Remote Code Intelligence for AI coding agents and editors
url = https://prod.codes
builddate = {now}
packager = Alexander Panasenko <alex@prod.codes>
size = {installed_size}
arch = {arch}
license = Apache-2.0
optdepend = systemd: for background user service
""".encode("utf-8")

install_script = b"""post_install() {
    if command -v systemctl >/dev/null 2>&1; then
        systemctl daemon-reload || true
    fi
    echo 'prod-code installed successfully. Run "prod-code status" or "prod-code --help".'
}
post_upgrade() {
    post_install
}
"""

with tarfile.open(out_path, "w:gz") as tar:
    # .PKGINFO
    ti = tarfile.TarInfo(name=".PKGINFO")
    ti.size = len(pkginfo)
    ti.mode = 0o644
    ti.mtime = now
    tar.addfile(ti, io.BytesIO(pkginfo))

    # .INSTALL
    ti_inst = tarfile.TarInfo(name=".INSTALL")
    ti_inst.size = len(install_script)
    ti_inst.mode = 0o644
    ti_inst.mtime = now
    tar.addfile(ti_inst, io.BytesIO(install_script))

    # /usr/bin/prod-code
    with open(client_bin, "rb") as f:
        data = f.read()
    ti_bin = tarfile.TarInfo(name="usr/bin/prod-code")
    ti_bin.size = len(data)
    ti_bin.mode = 0o755
    ti_bin.mtime = now
    tar.addfile(ti_bin, io.BytesIO(data))

    # /usr/lib/systemd/user/prod-code-gateway.service
    if os.path.exists(service_file):
        with open(service_file, "rb") as f:
            svc_data = f.read()
        ti_svc = tarfile.TarInfo(name="usr/lib/systemd/user/prod-code-gateway.service")
        ti_svc.size = len(svc_data)
        ti_svc.mode = 0o644
        ti_svc.mtime = now
        tar.addfile(ti_svc, io.BytesIO(svc_data))

    # /usr/share/doc/prod-code/README.md
    if os.path.exists(readme_file):
        with open(readme_file, "rb") as f:
            rd_data = f.read()
        ti_rd = tarfile.TarInfo(name="usr/share/doc/prod-code/README.md")
        ti_rd.size = len(rd_data)
        ti_rd.mode = 0o644
        ti_rd.mtime = now
        tar.addfile(ti_rd, io.BytesIO(rd_data))

print(f"✓ Arch package built: {out_path} ({os.path.getsize(out_path):,} bytes)")
PYEOF

fi
