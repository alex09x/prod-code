# Packaging, Distribution and Updates for prod-code

This document describes the distribution architecture, packaging tools, self-update mechanism, and publishing targets for `prod-code`.

## 1. Supported Package Formats

| Platform | Format | Tool / Script | Target Location |
|---|---|---|---|
| **macOS** | Disk Image (`.dmg`) | `scripts/packaging/package-macos-dmg.sh` | DMG containing `.pkg`, binary, README, uninstaller |
| **macOS** | Installer Package (`.pkg`) | `scripts/packaging/package-macos-pkg.sh` | `/usr/local/bin/prod-code` (ad-hoc codesigned) |
| **macOS / Linux** | Homebrew Formula | `scripts/packaging/generate-homebrew-formula.sh` | `brew install alex09x/tap/prod-code` |
| **Debian / Ubuntu** | Package (`.deb`) | `scripts/packaging/package-deb.py` | `/usr/bin/prod-code`, `/usr/bin/prod-code-server`, systemd unit |
| **Universal (sh)** | Curl installer | `scripts/packaging/install.sh` | Installs to `~/.local/bin/` or `/usr/local/bin/` |

---

## 2. Generating Release Packages

Run the master packaging script:

```bash
./scripts/packaging/build-all-packages.sh 0.3.19
```

This populates `dist/packages/` with:
- `prod-code-0.3.19-macOS.dmg`
- `prod-code-0.3.19-macOS.pkg`
- `prod-code_0.3.19_arm64.deb`
- `prod-code_0.3.19_amd64.deb`
- `install.sh`
- `SHA256SUMS`

---

## 3. Where to Publish

### A. GitHub Releases (Primary Distribution Host)
Every version tag (`v0.3.19`, etc.) receives:
1. Raw binaries for all 3 architectures:
   - `prod-code-aarch64-apple-darwin`
   - `prod-code-aarch64-unknown-linux-gnu`
   - `prod-code-x86_64-unknown-linux-gnu`
   - `prod-code-server-*`
2. Installable packages:
   - `prod-code-<version>-macOS.dmg`
   - `prod-code-<version>-macOS.pkg`
   - `prod-code_<version>_arm64.deb`
   - `prod-code_<version>_amd64.deb`
3. `SHA256SUMS` checksums file.

Upload via GitHub CLI:
```bash
gh release upload v0.3.19 dist/packages/* --repo alex09x/prod-code --clobber
```

### B. Homebrew Tap (`alex09x/homebrew-tap`)
1. Create or maintain repository `alex09x/homebrew-tap` on GitHub.
2. Place generated `Formula/prod-code.rb`:
```bash
./scripts/packaging/generate-homebrew-formula.sh 0.3.19 Formula/prod-code.rb
```
3. Users install and upgrade via:
```bash
brew tap alex09x/tap
brew install prod-code
brew upgrade prod-code
```

### C. Web Installer on `prod.codes`
The one-liner curl installer:
```bash
curl -fsSL https://prod.codes/install.sh | sh
```
The file `scripts/packaging/install.sh` is copied to `/Users/alex09x/Documents/workspace/prod.codes/public/install.sh`, served statically over Cloudflare with global CDN caching.

---

## 4. Self-Update System (`prod-code update`)

`prod-code` includes a built-in update subcommand:

- Check for updates:
  ```bash
  prod-code update --check
  ```
- Install latest update:
  ```bash
  prod-code update
  ```
- Force reinstall / update:
  ```bash
  prod-code update --force
  ```
- Install specific version:
  ```bash
  prod-code update --tag v0.3.19
  ```

### How It Works:
1. Detects current platform (`aarch64-apple-darwin`, `x86_64-unknown-linux-gnu`, etc.).
2. Detects if installed via Homebrew (`brew upgrade prod-code`) or APT/RPM package managers.
3. Fetches release metadata from GitHub API or `gh` CLI.
4. Downloads release binary into a temporary file next to `current_exe`.
5. Sets executable permissions (`chmod +x`) and performs ad-hoc codesigning on macOS.
6. Atomically swaps the executable (`rename`).
7. Triggers automatic MCP Hot Reload: any running `prod-code mcp` server detects binary mtime changes and notifies connected AI agents (`tools/list_changed`) without killing agent sessions!

---

## 5. Editor Integration (Zed)

### Option 1: Direct configuration in `~/.config/zed/settings.json`
```json
{
  "lsp": {
    "rust-analyzer": {
      "binary": { "path": "prod-code", "arguments": ["lsp", "--language", "rust"] }
    },
    "gopls": {
      "binary": { "path": "prod-code", "arguments": ["lsp", "--language", "go"] }
    },
    "clangd": {
      "binary": { "path": "prod-code", "arguments": ["lsp", "--language", "cpp"] }
    },
    "basedpyright": {
      "binary": { "path": "prod-code", "arguments": ["lsp", "--language", "python"] }
    },
    "vtsls": {
      "binary": { "path": "prod-code", "arguments": ["lsp", "--language", "typescript"] }
    }
  }
}
```

### Option 2: Dev Extension
In Zed: `Cmd+Shift+P` -> `zed: install dev extension` -> select `editors/zed`.
