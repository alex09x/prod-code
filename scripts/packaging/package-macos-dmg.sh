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

# 1. Build .pkg installer inside DMG root
PKG_SCRIPT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/package-macos-pkg.sh"
"$PKG_SCRIPT" --version "$VERSION" --client-bin "$CLIENT_BIN" --output "$DMG_ROOT/Install prod-code.pkg"

# 2. Put standalone binary
cp "$CLIENT_BIN" "$DMG_ROOT/prod-code"
chmod 755 "$DMG_ROOT/prod-code"

# 3. Quick-start documentation
cat << 'EOF' > "$DMG_ROOT/README.txt"
prod-code - Remote Code Intelligence for AI Coding Agents and Editors

Installation:
  Option 1: Double-click 'Install prod-code.pkg' to install to /usr/local/bin/prod-code.
  Option 2: Copy the 'prod-code' binary directly to your PATH (e.g. ~/.local/bin/ or ~/.cargo/bin/).

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

echo "Successfully built macOS DMG: $OUTPUT ($(stat -f%z "$OUTPUT" 2>/dev/null || stat -c%s "$OUTPUT") bytes)"
