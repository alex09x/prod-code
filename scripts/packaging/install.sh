#!/bin/sh
# prod-code universal installer
# Installs prod-code remote code-intelligence client for AI coding agents and editors.
# Usage: curl -fsSL https://prod.codes/install.sh | sh
set -e

REPO="alex09x/prod-code"

main() {
    OS="$(uname -s)"
    ARCH="$(uname -m)"

    case "$OS" in
        Darwin)
            OS_TARGET="apple-darwin"
            ;;
        Linux)
            OS_TARGET="unknown-linux-gnu"
            ;;
        *)
            echo "Error: Unsupported operating system: $OS" >&2
            exit 1
            ;;
    esac

    case "$ARCH" in
        x86_64|amd64)
            ARCH_TARGET="x86_64"
            ;;
        arm64|aarch64)
            ARCH_TARGET="aarch64"
            ;;
        *)
            echo "Error: Unsupported architecture: $ARCH" >&2
            exit 1
            ;;
    esac

    TARGET="${ARCH_TARGET}-${OS_TARGET}"
    echo "Detected platform: $TARGET"

    # Determine version to install
    if [ -n "$VERSION" ]; then
        TAG="v${VERSION#v}"
    else
        echo "Querying latest release from GitHub..."
        TAG=$(curl -sL "https://api.github.com/repos/${REPO}/releases/latest" | grep '"tag_name":' | head -1 | cut -d '"' -f 4)
        if [ -z "$TAG" ]; then
            TAG="v0.3.19"
        fi
    fi

    echo "Installing prod-code $TAG..."
    ASSET_URL="https://github.com/${REPO}/releases/download/${TAG}/prod-code-${TARGET}"

    # Determine installation directory
    if [ -w "/usr/local/bin" ]; then
        INSTALL_DIR="/usr/local/bin"
    elif [ -d "$HOME/.local/bin" ] || [ -w "$HOME" ]; then
        INSTALL_DIR="$HOME/.local/bin"
    elif [ -d "$HOME/.cargo/bin" ]; then
        INSTALL_DIR="$HOME/.cargo/bin"
    else
        INSTALL_DIR="/usr/local/bin"
    fi

    mkdir -p "$INSTALL_DIR"
    TMP_FILE="$(mktemp "${TMPDIR:-/tmp}/prod-code.XXXXXX")"

    echo "Downloading $ASSET_URL..."
    if ! curl -fL --progress-bar -o "$TMP_FILE" "$ASSET_URL"; then
        echo "Error: Failed to download release asset." >&2
        rm -f "$TMP_FILE"
        exit 1
    fi

    chmod +x "$TMP_FILE"

    # Ad-hoc codesign on macOS
    if [ "$OS" = "Darwin" ] && command -v codesign >/dev/null 2>&1; then
        codesign -s - -f "$TMP_FILE" >/dev/null 2>&1 || true
    fi

    DEST="$INSTALL_DIR/prod-code"
    mv -f "$TMP_FILE" "$DEST"
    chmod 755 "$DEST"

    echo ""
    echo "Successfully installed prod-code $TAG to $DEST!"
    "$DEST" --version

    # Check PATH
    case ":$PATH:" in
        *":$INSTALL_DIR:"*) ;;
        *)
            echo ""
            echo "Note: $INSTALL_DIR is not in your PATH."
            echo "Add it by running:"
            echo "    export PATH=\"$INSTALL_DIR:\$PATH\""
            ;;
    esac

    echo ""
    echo "Quick Start:"
    echo "  1. Test connection to build cluster:  prod-code status"
    echo "  2. Run as editor LSP server (Zed):    prod-code lsp --language rust"
    echo "  3. Run as agent MCP server:           prod-code mcp"
    echo "  4. Check for future updates:          prod-code update"
    echo ""
}

main "$@"
