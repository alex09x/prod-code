#!/usr/bin/env bash
# package-macos-dmg.sh - Builds a macOS .dmg disk image for prod-code containing .pkg installer, standalone binary and docs.
set -euo pipefail

usage() {
    echo "Usage: $0 --version <version> --client-bin <path> --output <path.dmg>"
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

DMG_ROOT="$TMP_DIR/dmg_root"
mkdir -p "$DMG_ROOT"

# 1. Put standalone client and server binaries
cp "$CLIENT_BIN" "$DMG_ROOT/prod-code"
chmod 755 "$DMG_ROOT/prod-code"

SERVER_BIN="${SERVER_BIN:-}"
if [[ -z "$SERVER_BIN" ]]; then
    for candidate in \
        "$(dirname "$CLIENT_BIN")/prod-code-server-aarch64-apple-darwin" \
        "$(dirname "$CLIENT_BIN")/prod-code-server"; do
        if [[ -f "$candidate" ]]; then
            SERVER_BIN="$candidate"
            break
        fi
    done
fi
if [[ -n "$SERVER_BIN" && -f "$SERVER_BIN" ]]; then
    cp "$SERVER_BIN" "$DMG_ROOT/prod-code-server"
    chmod 755 "$DMG_ROOT/prod-code-server"
fi

# 2. Put universal installer
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
if [[ -f "$SCRIPT_DIR/install.sh" ]]; then
    cp "$SCRIPT_DIR/install.sh" "$DMG_ROOT/install.sh"
    chmod 755 "$DMG_ROOT/install.sh"
fi

# 3. Quick-start documentation
cat << 'EOF' > "$DMG_ROOT/README.txt"
prod-code - Remote Code Intelligence for AI Coding Agents and Editors

Installation:
  Option 1: Copy 'prod-code' directly to your PATH (e.g. /usr/local/bin or ~/.local/bin).
  Option 2: Run './install.sh' in Terminal to install to ~/.local/bin.

Usage:
  prod-code status          - Probe cluster gateway health and latency
  prod-code lsp             - Run as editor language server (for Zed, VS Code, Neovim)
  prod-code mcp             - Run Model Context Protocol server for AI coding agents
  prod-code update          - Check for and install updates from GitHub releases

Editor Setup (Zed):
  In ~/.config/zed/settings.json:
  {
    "lsp": {
      "rust-analyzer": {
        "binary": { "path": "prod-code", "arguments": ["lsp", "--language", "rust"] }
      },
      "gopls": {
        "binary": { "path": "prod-code", "arguments": ["lsp", "--language", "go"] }
      }
    }
  }

For complete documentation visit: https://prod.codes
EOF

# 4. Uninstall helper script
cat << 'EOF' > "$DMG_ROOT/uninstall.sh"
#!/usr/bin/env bash
set -e
echo "Uninstalling prod-code..."
sudo rm -f /usr/local/bin/prod-code /usr/local/bin/prod-code-server
rm -f ~/.local/bin/prod-code ~/.local/bin/prod-code-server
sudo rm -rf ~/Library/Caches/prod-code
echo "prod-code removed successfully."
EOF
chmod 755 "$DMG_ROOT/uninstall.sh"

# 5. Build .dmg disk image via hdiutil
mkdir -p "$(dirname "$OUTPUT")"
hdiutil create \
    -volname "prod-code-${VERSION}" \
    -srcfolder "$DMG_ROOT" \
    -ov \
    -format UDZO \
    "$OUTPUT"

# 6. Sign DMG with Developer ID Application
if command -v codesign >/dev/null 2>&1; then
    SIGN_IDENTITY="${PROD_CODE_SIGN_IDENTITY:-Developer ID Application: Alexander Panasenko (284V2M3LN9)}"
    codesign --force --sign "$SIGN_IDENTITY" --timestamp "$OUTPUT" || true
fi

# 7. Notarize and staple if notary credentials are available
NOTARY_PROFILE="${PROD_CODE_NOTARY_PROFILE:-${TAKO_NOTARY_PROFILE:-tako}}"
if [[ "${SKIP_NOTARIZE:-0}" != "1" ]] && command -v xcrun >/dev/null 2>&1; then
    echo "Submitting DMG to Apple Notary Service (profile: $NOTARY_PROFILE)..."
    if xcrun notarytool history --keychain-profile "$NOTARY_PROFILE" >/dev/null 2>&1; then
        xcrun notarytool submit "$OUTPUT" --keychain-profile "$NOTARY_PROFILE" --wait
        echo "Stapling notarization ticket to DMG..."
        xcrun stapler staple "$OUTPUT"
        xcrun stapler validate "$OUTPUT"
    else
        echo "Warning: Notary profile '$NOTARY_PROFILE' not found; skipping notarization."
    fi
fi

echo "Successfully built macOS DMG: $OUTPUT ($(stat -f%z "$OUTPUT" 2>/dev/null || stat -c%s "$OUTPUT") bytes)"
