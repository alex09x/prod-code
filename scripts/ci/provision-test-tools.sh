#!/usr/bin/env bash
# prod-code — Remote code intelligence
# Copyright (c) 2026 Alexander Panasenko
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/prod-code
# SPDX-License-Identifier: MIT OR Apache-2.0
set -euo pipefail
: "${RUNNER_TEMP:?}" "${GITHUB_PATH:?}"
test "${GITHUB_ACTIONS:-}" = true
test "${RUNNER_ENVIRONMENT:-}" = github-hosted

bash "$(dirname "$0")/configure-test-profile.sh"

if test -e /proc/sys/kernel/apparmor_restrict_unprivileged_userns; then
    sudo sysctl -w kernel.apparmor_restrict_unprivileged_userns=0
fi
unshare --user --map-root-user true

packages=()
command -v rg >/dev/null 2>&1 || packages+=(ripgrep)
command -v clangd >/dev/null 2>&1 || packages+=(clangd)
if ((${#packages[@]})); then
    sudo apt-get update -qq
    sudo apt-get install -y --no-install-recommends "${packages[@]}"
fi

export GOTOOLCHAIN=go1.27.1
go_bin="$(go env GOPATH)/bin"
if ! test -x "$go_bin/gopls" || ! "$go_bin/gopls" version | grep -Fq 'gopls v0.23.0'; then
    go install golang.org/x/tools/gopls@v0.23.0
fi

tools="$RUNNER_TEMP/test-language-tools"
mkdir -p "$tools/bin"
if ! test -x "$tools/bin/sccache" || ! "$tools/bin/sccache" --version | grep -Fq 'sccache 0.17.0'; then
    archive="$RUNNER_TEMP/sccache-0.17.0.tar.gz"
    curl -fsSL --retry 2 \
        https://github.com/mozilla/sccache/releases/download/v0.17.0/sccache-v0.17.0-x86_64-unknown-linux-musl.tar.gz \
        -o "$archive"
    printf '%s  %s\n' 67c4a96dd237c1f518f6b36083f270f9976d516f1e57fce891755ea782e50006 "$archive" | sha256sum -c -
    tar -xzf "$archive" -O sccache-v0.17.0-x86_64-unknown-linux-musl/sccache > "$tools/bin/sccache"
    chmod 755 "$tools/bin/sccache"
    rm -f "$archive"
fi

if ! test -x "$tools/node_modules/.bin/basedpyright" || ! test -x "$tools/node_modules/.bin/typescript-language-server"; then
    npm install --prefix "$tools" --no-package-lock --no-audit --no-fund --ignore-scripts \
        typescript@7.0.2 typescript-language-server@6.0.0 basedpyright@1.40.1
fi

if ! test -x "$tools/python/bin/ruff" || ! "$tools/python/bin/ruff" --version | grep -Fq 'ruff 0.16.8'; then
    rm -rf "$tools/python"
    python3 -m venv "$tools/python"
    "$tools/python/bin/pip" install --disable-pip-version-check ruff==0.16.8
fi

printf '%s\n' "$go_bin" "$tools/bin" "$tools/node_modules/.bin" "$tools/python/bin" >> "$GITHUB_PATH"
"$tools/bin/sccache" --version
"$go_bin/gopls" version
"$tools/node_modules/.bin/basedpyright" --version
"$tools/node_modules/.bin/typescript-language-server" --version
"$tools/python/bin/ruff" --version
