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
timeout 60 unshare --user --map-root-user true

# Every network step is bounded and says what it is doing: an apt mirror that stopped answering
# held three shards silent for the job's whole 90 minutes, after the line above.
apt_options=(-o Acquire::Retries=3 -o Acquire::http::Timeout=30 -o Acquire::https::Timeout=30
    -o DPkg::Lock::Timeout=120)
packages=()
command -v rg >/dev/null 2>&1 || packages+=(ripgrep)
command -v clangd >/dev/null 2>&1 || packages+=(clangd)
if ((${#packages[@]})); then
    echo "installing ${packages[*]} with apt"
    sudo timeout 600 apt-get "${apt_options[@]}" update
    sudo timeout 600 apt-get "${apt_options[@]}" install -y --no-install-recommends "${packages[@]}"
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

# Rust Intelligence, the gateway's default Rust server: the released ridx and ridx-driver. The
# driver links the toolchain's rustc_driver, so Rust 1.98.1 with rustc-dev is needed at run
# time too (ridx gives the driver that sysroot); the binaries are cached with the other tools.
ridx_version=0.3.12
ridx_archive=ridx-x86_64-unknown-linux-gnu
ridx_sha256=1933242c24313cf23d4e506c03cff287e5e3f81115a44a4938ae52c9b97b00d4
rustup toolchain install 1.98.1 --profile minimal --component rustc-dev --component rust-src
if ! test -x "$tools/bin/ridx-driver" || ! "$tools/bin/ridx" --version | grep -Fq "ridx $ridx_version "; then
    archive="$RUNNER_TEMP/$ridx_archive.tar.gz"
    curl -fsSL --retry 2 --max-time 300 \
        "https://github.com/alex09x/rust-intelligence/releases/download/v$ridx_version/$ridx_archive.tar.gz" \
        -o "$archive"
    printf '%s  %s\n' "$ridx_sha256" "$archive" | sha256sum -c -
    tar -xzf "$archive" -C "$RUNNER_TEMP"
    install -m 755 "$RUNNER_TEMP/$ridx_archive/ridx" "$RUNNER_TEMP/$ridx_archive/ridx-driver" "$tools/bin/"
    rm -rf "$archive" "$RUNNER_TEMP/$ridx_archive"
fi

printf '%s\n' "$go_bin" "$tools/bin" "$tools/node_modules/.bin" "$tools/python/bin" >> "$GITHUB_PATH"
"$tools/bin/ridx" --version
LD_LIBRARY_PATH="$(rustc +1.98.1 --print sysroot)/lib" "$tools/bin/ridx-driver" --version
"$tools/bin/sccache" --version
"$go_bin/gopls" version
"$tools/node_modules/.bin/basedpyright" --version
"$tools/node_modules/.bin/typescript-language-server" --version
"$tools/python/bin/ruff" --version
