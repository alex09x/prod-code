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
if test -e /proc/sys/kernel/apparmor_restrict_unprivileged_userns; then
    sudo sysctl -w kernel.apparmor_restrict_unprivileged_userns=0
fi
unshare --user --map-root-user true
if ! command -v rg >/dev/null 2>&1; then
    sudo apt-get update -qq
    sudo apt-get install -y --no-install-recommends ripgrep
fi
export GOTOOLCHAIN=go1.27.1
go_bin="$(go env GOPATH)/bin"
if ! test -x "$go_bin/gopls" || ! "$go_bin/gopls" version | grep -Fq 'gopls v0.23.0'; then
    go install golang.org/x/tools/gopls@v0.23.0
fi
tools="$RUNNER_TEMP/test-language-tools"
npm install --prefix "$tools" --no-package-lock --no-audit --no-fund --ignore-scripts \
    typescript@7.0.2 typescript-language-server@6.0.0 basedpyright@1.40.1
python3 -m venv "$tools/python"
"$tools/python/bin/pip" install --disable-pip-version-check ruff==0.16.8
printf '%s\n' "$go_bin" "$tools/node_modules/.bin" "$tools/python/bin" >> "$GITHUB_PATH"
"$go_bin/gopls" version
"$tools/node_modules/.bin/basedpyright" --version
"$tools/node_modules/.bin/typescript-language-server" --version
"$tools/python/bin/ruff" --version
